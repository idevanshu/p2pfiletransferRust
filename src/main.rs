use clap::{Arg, Command};
use indicatif::{ProgressBar, ProgressStyle};
use memmap2::MmapOptions;
use num_cpus;
use socket2::{Socket, TcpKeepalive};
use std::error::Error;
use std::ffi::OsStr;
use std::io;
use std::net::TcpStream as StdTcpStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use async_recursion::async_recursion;
use tokio::fs::{self, File};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;

const COPY_BUFFER_SIZE: usize = 256 * 1024; //256KByte
const TCP_BUFFER_SIZE: usize = 2 * 1024 * 1024; //2MByte
const MAX_COMP_LEN: usize = 240;
const KEEPALIVE_TIME: Duration = Duration::from_secs(60);

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let matches = Command::new("Fast File Transfer")
        .version("3.1")
        .author("github.com/idevanshu")
        .arg(
            Arg::new("mode")
                .short('m')
                .long("mode")
                .required(true)
                .value_parser(["send", "receive"]),
        )
        .arg(
            Arg::new("file")
                .short('f')
                .long("file")
                .required_if_eq("mode", "send"),
        )
        .arg(
            Arg::new("address")
                .short('a')
                .long("address")
                .required_if_eq("mode", "receive"),
        )
        .get_matches();

    match matches.get_one::<String>("mode").map(|s| s.as_str()) {
        Some("send") => send(matches.get_one::<String>("file").unwrap()).await,
        Some("receive") => receive(matches.get_one::<String>("address").unwrap()).await,
        _ => unreachable!(),
    }
}

async fn send(path: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
    let listener = TcpListener::bind("0.0.0.0:8000").await?;
    println!("Sender ready at: {}", listener.local_addr()?);
    let semaphore = Arc::new(Semaphore::new(num_cpus::get()));
    
    loop {
        let (socket, addr) = listener.accept().await?;
        println!("Receiver connected: {}", addr);
        let permit = semaphore.clone().acquire_owned().await?;
        let path = path.to_string();
        
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(e) = handle_send(socket, &path).await {
                eprintln!("Transfer failed: {:#}", e);
            }
        });
    }
}

async fn handle_send(socket: TcpStream, path: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
    let socket = configure_socket(socket)?;
    let mut writer = BufWriter::with_capacity(COPY_BUFFER_SIZE, socket);
    let p = Path::new(path);
    
    if p.is_file() {
        writer.write_all(&[0u8]).await?;
        send_file_metadata(&mut writer, p).await?;
        send_single_file(&mut writer, p).await?;
    } else {
        writer.write_all(&[1u8]).await?;
        send_folder_metadata(&mut writer, p).await?;
        let files = collect_files_recursive(p, p).await?;
        writer.write_all(&(files.len() as u32).to_le_bytes()).await?;
        
        for (abs, rel) in files {
            send_file_metadata(&mut writer, &rel).await?;
            send_single_file(&mut writer, &abs).await?;
        }
    }
    
    Ok(())
}

async fn send_file_metadata(
    writer: &mut BufWriter<TcpStream>,
    path: &Path,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let file_name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Invalid filename"))?
        .to_string_lossy()
        .replace('\0', "");
    
    let name_bytes = file_name.as_bytes();
    writer.write_all(&(name_bytes.len() as u16).to_le_bytes()).await?;
    writer.write_all(name_bytes).await?;
    Ok(())
}

async fn send_folder_metadata(
    writer: &mut BufWriter<TcpStream>,
    path: &Path,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let folder_name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Invalid folder name"))?
        .to_string_lossy()
        .replace('\0', "");
    
    let name_bytes = folder_name.as_bytes();
    writer.write_all(&(name_bytes.len() as u16).to_le_bytes()).await?;
    writer.write_all(name_bytes).await?;
    Ok(())
}

async fn send_single_file(
    writer: &mut BufWriter<TcpStream>,
    file_path: &Path,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let file = File::open(file_path).await?;
    let metadata = file.metadata().await?;
    let mmap = unsafe { MmapOptions::new().map(&file)? };
    
    writer.write_all(&metadata.len().to_le_bytes()).await?;
    let progress = create_progress_bar(metadata.len(), file_path);
    let mut sent = 0;
    
    while sent < mmap.len() {
        let chunk = mmap[sent..].chunks(COPY_BUFFER_SIZE).next().unwrap();
        writer.write_all(chunk).await?;
        sent += chunk.len();
        progress.set_position(sent as u64);
    }
    
    writer.flush().await?;
    progress.finish_with_message("✓");
    Ok(())
}

#[async_recursion]
async fn collect_files_recursive(
    root: &Path,
    base: &Path,
) -> io::Result<Vec<(PathBuf, PathBuf)>> {
    let mut files = Vec::new();
    let mut entries = fs::read_dir(root).await?;
    
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        
        if path.is_file() {
            let rel = path.strip_prefix(base)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
                .to_path_buf();
            files.push((path, rel));
        } else if path.is_dir() {
            // Box the recursive call automatically with the macro.
            files.extend(collect_files_recursive(&path, base).await?);
        }
    }
    
    Ok(files)
}

async fn receive(address: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
    let socket = TcpStream::connect(address).await?;
    let socket = configure_socket(socket)?;
    let mut reader = BufReader::with_capacity(COPY_BUFFER_SIZE, socket);
    let mut mode = [0u8; 1];
    reader.read_exact(&mut mode).await?;
    
    let dest_root = "received";
    fs::create_dir_all(dest_root).await?;
    
    if mode[0] == 0 {
        let file_name = read_filename(&mut reader).await?;
        let dest = sanitize_path(&Path::new(dest_root).join(file_name));
        receive_single_file(&mut reader, &dest).await?;
    } else {
        let folder_name = read_filename(&mut reader).await?;
        let base_folder = sanitize_path(&Path::new(dest_root).join(folder_name));
        fs::create_dir_all(&base_folder).await?;
        
        let mut count_buf = [0u8; 4];
        reader.read_exact(&mut count_buf).await?;
        let count = u32::from_le_bytes(count_buf);
        
        for _ in 0..count {
            let rel_path = read_filename(&mut reader).await?;
            let full_path = sanitize_path(&base_folder.join(rel_path));
            
            if let Some(parent) = full_path.parent() {
                fs::create_dir_all(parent).await?;
            }
            
            receive_single_file(&mut reader, &full_path).await?;
        }
    }
    
    Ok(())
}

async fn read_filename(reader: &mut BufReader<TcpStream>) -> Result<String, Box<dyn Error + Send + Sync>> {
    let mut len_buf = [0u8; 2];
    reader.read_exact(&mut len_buf).await?;
    let name_len = u16::from_le_bytes(len_buf) as usize;
    
    let mut name_bytes = vec![0u8; name_len];
    reader.read_exact(&mut name_bytes).await?;
    
    Ok(String::from_utf8_lossy(&name_bytes)
        .replace('\0', "")
        .replace(|c: char| c.is_control(), "")
        .trim()
        .to_string())
}

async fn receive_single_file(
    reader: &mut BufReader<TcpStream>,
    dest: &Path,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut size_buf = [0u8; 8];
    reader.read_exact(&mut size_buf).await?;
    let file_size = u64::from_le_bytes(size_buf);
    
    let mut file = BufWriter::with_capacity(COPY_BUFFER_SIZE, File::create(dest).await?);
    let progress = create_progress_bar(file_size, dest);
    let mut received = 0;
    
    while received < file_size {
        let chunk = reader.fill_buf().await?;
        if chunk.is_empty() { break; }
        
        let bytes_to_write = chunk.len().min((file_size - received) as usize);
        file.write_all(&chunk[..bytes_to_write]).await?;
        received += bytes_to_write as u64;
        progress.set_position(received);
        reader.consume(bytes_to_write);
    }
    
    file.flush().await?;
    progress.finish_with_message("✓");
    Ok(())
}

fn configure_socket(socket: TcpStream) -> io::Result<TcpStream> {
    let std_stream: StdTcpStream = socket.into_std()?;
    std_stream.set_nodelay(true)?;
    
    let socket2 = Socket::from(std_stream);
    socket2.set_send_buffer_size(TCP_BUFFER_SIZE)?;
    socket2.set_recv_buffer_size(TCP_BUFFER_SIZE)?;
    
    let keepalive = TcpKeepalive::new()
        .with_time(KEEPALIVE_TIME);
    socket2.set_tcp_keepalive(&keepalive)?;
    
    TcpStream::from_std(socket2.into())
}

fn sanitize_path(path: &Path) -> PathBuf {
    path.components()
        .map(|comp| {
            let comp_str = comp.as_os_str().to_string_lossy();
            let filtered = comp_str
                .replace(|c: char| c.is_control() || "<>:\"/\\|?*".contains(c), "_")
                .trim()
                .chars()
                .take(MAX_COMP_LEN)
                .collect::<String>();
            OsStr::new(&filtered).to_os_string()
        })
        .collect()
}

fn create_progress_bar(len: u64, path: &Path) -> ProgressBar {
    let name = path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");
    
    ProgressBar::new(len).with_style(
        ProgressStyle::default_bar()
            .template(&format!("{{spinner:.green}} [{name}] {{elapsed_precise}} {{bar:40}} {{bytes}}/{{total_bytes}} ({{eta}})"))
            .unwrap()
    )
}