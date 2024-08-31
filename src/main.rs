use clap::{Arg, Command};
use std::error::Error;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, Mutex};
use tokio::task;
use indicatif::{ProgressBar, ProgressStyle};
use num_cpus;

const BUFFER_SIZE: usize = 64 * 1024; // 64 KB buffer size

fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let num_threads = num_cpus::get();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(num_threads)
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async_main())
}

async fn async_main() -> Result<(), Box<dyn Error + Send + Sync>> {
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

async fn send_file(file_path: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
    let file_name = file_path.split('/').last().unwrap_or("file").to_string();
    let listener = TcpListener::bind("0.0.0.0:8000").await?;
    println!("Listening on 0.0.0.0:8000");

    let semaphore = Arc::new(Semaphore::new(num_cpus::get())); // Limit concurrent connections

    while let Ok((socket, _)) = listener.accept().await {
        let semaphore = semaphore.clone();
        let file_path = file_path.to_string();
        let file_name = file_name.clone();

        tokio::spawn(async move {
            let _permit = semaphore.acquire().await.unwrap();

            if let Err(e) = handle_connection(socket, file_path, file_name).await {
                eprintln!("Connection handling failed: {}", e);
            }
        });
    }

    Ok(())
}

async fn handle_connection(mut socket: TcpStream, file_path: String, file_name: String) -> Result<(), Box<dyn Error + Send + Sync>> {
    let file = tokio::fs::File::open(&file_path).await?;
    let file_size = file.metadata().await?.len();
    let _buf = vec![0; BUFFER_SIZE];
    let file_name_bytes = file_name.as_bytes();

    socket.write_all(&file_size.to_le_bytes()).await?;
    socket.write_all(file_name_bytes).await?;
    socket.write_all(&[b'\n']).await?;

    let chunks = (file_size + BUFFER_SIZE as u64 - 1) / BUFFER_SIZE as u64;
    let progress = Arc::new(ProgressBar::new(chunks));
    progress.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})")
            .unwrap()
            .progress_chars("#>-"),
    );

    let file = Arc::new(Mutex::new(file));
    let socket = Arc::new(Mutex::new(socket));

    let mut tasks = vec![];

    for _ in 0..num_cpus::get() {
        let file = file.clone();
        let socket = socket.clone();
        let progress = progress.clone();

        tasks.push(task::spawn(async move {
            let mut local_buf = vec![0; BUFFER_SIZE];
            loop {
                let bytes_read = {
                    let mut file = file.lock().await;
                    file.read(&mut local_buf).await?
                };

                if bytes_read == 0 {
                    break;
                }

                {
                    let mut socket = socket.lock().await;
                    socket.write_all(&local_buf[..bytes_read]).await?;
                }

                progress.inc(1);
            }
            Ok::<_, Box<dyn Error + Send + Sync>>(())
        }));
    }

    for task in tasks {
        task.await??;
    }

    progress.finish_with_message("File sent successfully!");

    Ok(())
}

async fn receive_file(address: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
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
    let file = tokio::fs::File::create(&file_name).await?;

    println!("Receiving file as: {}", file_name);

    let progress_bar = Arc::new(ProgressBar::new(file_size));
    progress_bar.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
            .unwrap()
            .progress_chars("#>-"),
    );

    let file = Arc::new(Mutex::new(file));
    let stream = Arc::new(Mutex::new(stream));

    let mut tasks = vec![];

    for _ in 0..num_cpus::get() {
        let file = file.clone();
        let stream = stream.clone();
        let progress_bar = progress_bar.clone();

        tasks.push(task::spawn(async move {
            let mut local_buf = vec![0; BUFFER_SIZE];
            loop {
                let bytes_read = {
                    let mut stream = stream.lock().await;
                    stream.read(&mut local_buf).await?
                };

                if bytes_read == 0 {
                    break;
                }

                {
                    let mut file = file.lock().await;
                    file.write_all(&local_buf[..bytes_read]).await?;
                }

                progress_bar.inc(bytes_read as u64);
            }
            Ok::<_, Box<dyn Error + Send + Sync>>(())
        }));
    }

    for task in tasks {
        task.await??;
    }

    progress_bar.finish_with_message("File received successfully!");

    Ok(())
}
