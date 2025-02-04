use clap::{Arg, Command};
use indicatif::{ProgressBar, ProgressStyle};
use memmap2::MmapOptions;
use num_cpus;
use socket2::Socket;
use std::error::Error;
use std::fs;
use std::io;
use std::net::TcpStream as StdTcpStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;

const COPY_BUFFER_SIZE: usize = 256 * 1024; //256KByte
const TCP_BUFFER_SIZE: usize = 2 * 1024 * 1024; //2MByte
const MAX_COMP_LEN: usize = 240;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let matches = Command::new("Fast File Transfer")
        .version("3.0")
        .author("github.com/idevanshu")
        .arg(
            Arg::new("mode")
                .short('m')
                .long("mode")
                .required(true)
                .value_parser(["send", "receive"]),
        )
        .arg(
            Arg::new("f")
                .short('f')
                .long("f")
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
        Some("send") => send(matches.get_one::<String>("f").unwrap()).await,
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
                eprintln!("Error: {}", e);
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
        let file_name = p
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned()
            .replace("\0", "");
        let name_bytes = file_name.as_bytes();
        let name_len = name_bytes.len() as u16;
        writer.write_all(&name_len.to_le_bytes()).await?;
        writer.write_all(name_bytes).await?;
        send_single_file(&mut writer, p).await?;
    } else {
        writer.write_all(&[1u8]).await?;
        let folder_name = p
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned()
            .replace("\0", "");
        writer.write_all(folder_name.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        let files = collect_files_recursive(p, p)?;
        writer.write_all(&(files.len() as u32).to_le_bytes()).await?;
        for (abs, rel) in files {
            let rel_str = rel.to_string_lossy().into_owned().replace("\0", "");
            let rel_len = rel_str.len() as u16;
            writer.write_all(&rel_len.to_le_bytes()).await?;
            writer.write_all(rel_str.as_bytes()).await?;
            send_single_file(&mut writer, &abs).await?;
        }
    }
    Ok(())
}

async fn send_single_file(
    writer: &mut BufWriter<TcpStream>,
    file_path: &Path,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let file = File::open(file_path).await?;
    let metadata = file.metadata().await?;
    let mmap = unsafe { MmapOptions::new().map(&file)? };
    writer.write_all(&(metadata.len()).to_le_bytes()).await?;
    let file_name = file_path.file_name().unwrap().to_string_lossy();
    let progress = ProgressBar::new(metadata.len());
    progress.set_style(
        ProgressStyle::default_bar()
            .template(
                &format!(
                    "{{spinner:.green}} [{}] {{elapsed_precise}} {{bar:40}} {{bytes}}/{{total_bytes}} ({{eta}})",
                    file_name
                )
            )
            .unwrap(),
    );
    let mut sent = 0;
    while sent < mmap.len() {
        let chunk = std::cmp::min(COPY_BUFFER_SIZE, mmap.len() - sent);
        writer.write_all(&mmap[sent..sent + chunk]).await?;
        writer.flush().await?;
        sent += chunk;
        progress.set_position(sent as u64);
    }
    progress.finish_with_message("File sent");
    Ok(())
}

fn collect_files_recursive(root: &Path, base: &Path) -> io::Result<Vec<(PathBuf, PathBuf)>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let p = entry.path();
        if p.is_file() {
            let rel = p.strip_prefix(base).unwrap().to_path_buf();
            files.push((p, rel));
        } else if p.is_dir() {
            let sub = collect_files_recursive(&p, base)?;
            files.extend(sub);
        }
    }
    Ok(files)
}

async fn receive(address: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
    let dest_root = "received";
    fs::create_dir_all(dest_root)?;
    let socket = TcpStream::connect(address).await?;
    let socket = configure_socket(socket)?;
    let mut reader = BufReader::with_capacity(COPY_BUFFER_SIZE, socket);
    let mut mode = [0u8; 1];
    reader.read_exact(&mut mode).await?;
    if mode[0] == 0 {
        let mut name_len_buf = [0u8; 2];
        reader.read_exact(&mut name_len_buf).await?;
        let name_len = u16::from_le_bytes(name_len_buf) as usize;
        let mut name_bytes = vec![0u8; name_len];
        reader.read_exact(&mut name_bytes).await?;
        let file_name = String::from_utf8_lossy(&name_bytes)
            .into_owned()
            .replace("\0", "");
        let dest = sanitize_path(&Path::new(dest_root).join(file_name));
        receive_single_file(&mut reader, dest.to_str().unwrap()).await?;
    } else {
        let mut folder_name = String::new();
        reader.read_line(&mut folder_name).await?;
        let folder_name = folder_name.trim_end().replace("\0", "");
        let base_folder = sanitize_path(&Path::new(dest_root).join(folder_name));
        fs::create_dir_all(&base_folder)?;
        let mut count_buf = [0u8; 4];
        reader.read_exact(&mut count_buf).await?;
        let count = u32::from_le_bytes(count_buf);
        for _ in 0..count {
            let mut rel_len_buf = [0u8; 2];
            reader.read_exact(&mut rel_len_buf).await?;
            let rel_len = u16::from_le_bytes(rel_len_buf) as usize;
            let mut rel_bytes = vec![0u8; rel_len];
            reader.read_exact(&mut rel_bytes).await?;
            let rel_path = String::from_utf8_lossy(&rel_bytes)
                .into_owned()
                .replace("\0", "");
            let full_path = sanitize_path(&base_folder.join(rel_path));
            if let Some(parent) = full_path.parent() {
                fs::create_dir_all(parent)?;
            }
            receive_single_file(&mut reader, full_path.to_str().unwrap()).await?;
        }
    }
    Ok(())
}

async fn receive_single_file(
    reader: &mut BufReader<TcpStream>,
    dest: &str,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut size_buf = [0u8; 8];
    reader.read_exact(&mut size_buf).await?;
    let file_size = u64::from_le_bytes(size_buf);
    let mut file = BufWriter::with_capacity(COPY_BUFFER_SIZE, File::create(dest).await?);
    let file_name = Path::new(dest).file_name().unwrap().to_string_lossy();
    let progress = ProgressBar::new(file_size);
    progress.set_style(
        ProgressStyle::default_bar()
            .template(
                &format!(
                    "{{spinner:.green}} [{}] {{elapsed_precise}} {{bar:40}} {{bytes}}/{{total_bytes}} ({{eta}})",
                    file_name
                )
            )
            .unwrap(),
    );
    let mut received = 0;
    let mut buffer = vec![0u8; COPY_BUFFER_SIZE];
    while received < file_size {
        let read_size = reader.read(&mut buffer).await?;
        if read_size == 0 {
            break;
        }
        file.write_all(&buffer[..read_size]).await?;
        file.flush().await?;
        received += read_size as u64;
        progress.set_position(received);
    }
    progress.finish_with_message("File received");
    Ok(())
}

fn configure_socket(socket: TcpStream) -> io::Result<TcpStream> {
    let std_stream: StdTcpStream = socket.into_std()?;
    std_stream.set_nodelay(true)?;
    let socket2 = Socket::from(std_stream);
    socket2.set_send_buffer_size(TCP_BUFFER_SIZE)?;
    socket2.set_recv_buffer_size(TCP_BUFFER_SIZE)?;
    TcpStream::from_std(socket2.into())
}

fn sanitize_path(path: &Path) -> PathBuf {
    let mut new_path = PathBuf::new();
    for comp in path.components() {
        let comp_os = comp.as_os_str().to_string_lossy();
        let comp_str = comp_os.trim();
        let safe_comp = if comp_str.len() > MAX_COMP_LEN {
            comp_str.chars().take(MAX_COMP_LEN).collect::<String>()
        } else {
            comp_str.to_string()
        };
        new_path.push(safe_comp);
    }
    new_path
}
