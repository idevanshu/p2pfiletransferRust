use clap::{Arg, Command};
use std::error::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let matches = Command::new("File Transfer")
        .version("1.0")
        .author("Your Name")
        .about("Send and receive files using TCP")
        .arg(Arg::new("mode")
            .short('m')
            .long("mode")
            .help("Mode: send or receive")
            .required(true)
            .value_parser(clap::value_parser!(String))) 
        .arg(Arg::new("file")
            .short('f')
            .long("file")
            .help("File path for sending")
            .value_parser(clap::value_parser!(String))) 
        .arg(Arg::new("address")
            .short('a')
            .long("address")
            .help("Address for receiving")
            .value_parser(clap::value_parser!(String))) 
        .get_matches();

    let mode = matches.get_one::<String>("mode").unwrap();

    match mode.as_str() {
        "send" => {
            let file_path = matches.get_one::<String>("file").unwrap();
            send_file(file_path).await?;
        },
        "receive" => {
            let address = matches.get_one::<String>("address").unwrap();
            receive_file(address).await?;
        },
        _ => println!("Invalid mode. Use 'send' or 'receive'."),
    }

    Ok(())
}

async fn send_file(file_path: &str) -> Result<(), Box<dyn Error>> {
    let file_name = file_path.split('/').last().unwrap_or("file").to_string();
    let listener = TcpListener::bind("0.0.0.0:8000").await?;
    println!("Listening on 0.0.0.0:8000");

    loop {
        let (mut socket, _) = listener.accept().await?;
        let file_path = file_path.to_string();
        let file_name = file_name.clone();

        tokio::spawn(async move {
            let mut file = tokio::fs::File::open(file_path).await.unwrap();
            let mut buf = vec![0; 4096];  // Larger buffer size
            let mut file_name_bytes = file_name.as_bytes().to_vec();
            file_name_bytes.push(b'\n');

            // Send file name
            if let Err(e) = socket.write_all(&file_name_bytes).await {
                eprintln!("Failed to send file name: {}", e);
                return;
            }

            // Send file data
            loop {
                let n = file.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                if let Err(e) = socket.write_all(&buf[..n]).await {
                    eprintln!("Failed to send file data: {}", e);
                    break;
                }
            }
        });
    }
}

async fn receive_file(address: &str) -> Result<(), Box<dyn Error>> {
    let mut stream = TcpStream::connect(address).await?;
    let mut buf = vec![0; 4096];  // Larger buffer size
    let mut file_name_buf = Vec::new();

    // Read the filename
    loop {
        let n = stream.read(&mut buf).await?;
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

    let file_name = String::from_utf8(file_name_buf).unwrap_or_else(|_| "received_file".to_string());
    let mut file = tokio::fs::File::create(file_name.clone()).await?;

    println!("Receiving file as: {}", file_name);

    // Receive file data
    loop {
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            println!("File reception completed.");
            break;
        }
        if let Err(e) = file.write_all(&buf[..n]).await {
            eprintln!("Failed to write data: {}", e);
            break;
        }
    }

    Ok(())
}
