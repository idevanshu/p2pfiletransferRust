use clap::{Arg, Command};
use std::error::Error;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use indicatif::{ProgressBar, ProgressStyle};

const MAX_CONNECTIONS: usize = 32;
const BUFFER_SIZE: usize = 64 * 1024; // 64 KB buffer size

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let matches = Command::new("File Transfer")
        .version("1.0")
        .author("github.com/idevanshu")
        .about("Send and receive files using TCP")
        .arg(
            Arg::new("mode")
                .short('m')
                .long("mode")
                .help("Mode: send or receive")
                .required(true)
                .value_parser(clap::value_parser!(String)),
        )
        .arg(
            Arg::new("file")
                .short('f')
                .long("file")
                .help("File path for sending")
                .value_parser(clap::value_parser!(String)),
        )
        .arg(
            Arg::new("address")
                .short('a')
                .long("address")
                .help("Address for receiving")
                .value_parser(clap::value_parser!(String)),
        )
        .get_matches();

    let mode = matches.get_one::<String>("mode").unwrap().as_str();

    match mode {
        "send" => {
            let file_path = matches.get_one::<String>("file").unwrap();
            send_file(file_path).await?;
        }
        "receive" => {
            let address = matches.get_one::<String>("address").unwrap();
            receive_file(address).await?;
        }
        _ => eprintln!("Invalid mode. Use 'send' or 'receive'."),
    }

    Ok(())
}

async fn send_file(file_path: &str) -> Result<(), Box<dyn Error>> {
    let file_name = file_path.split('/').last().unwrap_or("file").to_string();
    let listener = TcpListener::bind("0.0.0.0:8000").await?;
    println!("Listening on 0.0.0.0:8000");

    let semaphore = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let mut active_connections = 0;

    while let Ok((socket, _)) = listener.accept().await {
        let semaphore = semaphore.clone();
        let file_path = file_path.to_string();
        let file_name = file_name.clone();

        tokio::spawn(async move {
            let _permit = semaphore.acquire().await.unwrap();
            active_connections += 1;
            println!("Active connections: {}", active_connections);

            if let Err(e) = handle_connection(socket, file_path, file_name).await {
                eprintln!("Connection handling failed: {}", e);
            }

            active_connections -= 1;
            println!("Active connections: {}", active_connections);
        });
    }

    Ok(())
}

async fn handle_connection(mut socket: TcpStream, file_path: String, file_name: String) -> Result<(), Box<dyn Error>> {
    let mut file = tokio::fs::File::open(&file_path).await?;
    let file_size = file.metadata().await?.len();
    let mut buf = vec![0; BUFFER_SIZE];
    let file_name_bytes = file_name.as_bytes();

    socket.write_all(&file_size.to_le_bytes()).await?;
    socket.write_all(file_name_bytes).await?;
    socket.write_all(&[b'\n']).await?;

    while let Ok(n) = file.read(&mut buf).await {
        if n == 0 {
            break;
        }
        socket.write_all(&buf[..n]).await?;
    }

    Ok(())
}

async fn receive_file(address: &str) -> Result<(), Box<dyn Error>> {
    let address = address.strip_prefix("tcp://").unwrap_or(address);
    let mut stream = TcpStream::connect(address).await?;
    let mut buf = vec![0; BUFFER_SIZE];
    let mut file_name_buf = Vec::new();

    stream.read_exact(&mut buf[..8]).await?;
    let file_size = u64::from_le_bytes(buf[..8].try_into().unwrap());

    while let Ok(n) = stream.read(&mut buf).await {
        if n == 0 {
            break;
        }
        if let Some(index) = buf.iter().position(|&x| x == b'\n') {
            file_name_buf.extend_from_slice(&buf[..index]);
            break;
        } else {
            file_name_buf.extend_from_slice(&buf[..n]);
        }
    }

    let file_name = String::from_utf8(file_name_buf).unwrap_or("received_file".to_string());
    let mut file = tokio::fs::File::create(&file_name).await?;

    println!("Receiving file as: {}", file_name);

    let progress_bar = ProgressBar::new(file_size);
    progress_bar.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
            .unwrap()
            .progress_chars("#>-"),
    );

    let mut total_bytes_received = 0;
    while let Ok(n) = stream.read(&mut buf).await {
        if n == 0 {
            println!("File reception completed.");
            break;
        }
        file.write_all(&buf[..n]).await?;
        total_bytes_received += n as u64;
        progress_bar.set_position(total_bytes_received);
    }

    progress_bar.finish_with_message("File received successfully!");
    Ok(())
}
