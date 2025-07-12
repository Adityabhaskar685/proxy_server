use std::env;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

const LISTEN_IP: &str = "0.0.0.0";
const LISTEN_PORT: u16 = 9999;
const TARGET_PORT: u16 = 80;
const BUFFER_SIZE: usize = 4096;

fn handle_client(
    mut client_socket: TcpStream,
    target_ip: String,
    target_port: u16,
) -> io::Result<()> {
    let mut remote_socket = TcpStream::connect((target_ip.as_str(), target_port))?;
    println!(
        "[+] connected to target server {}:{}",
        target_ip, target_port
    );

    // Streams clones for bidirectional forwarding
    let mut client_read = client_socket.try_clone()?;
    let mut remote_write = remote_socket.try_clone()?;
    let mut remote_read = remote_socket.try_clone()?;
    let mut client_write = client_socket.try_clone()?;

    // Forward data from client to remote server
    let client_to_remote =
        thread::spawn(move || forward_data(&mut client_read, &mut remote_write, "Client->Remote"));

    // Forward data from remote server to client
    let remote_to_client =
        thread::spawn(move || forward_data(&mut remote_read, &mut client_write, "Remote->Client"));

    let _ = client_to_remote.join();
    let _ = remote_to_client.join();

    println!("[+] Connection closed");
    Ok(())
}

fn forward_data(src: &mut TcpStream, dst: &mut TcpStream, direction: &str) {
    let mut buffer = [0; BUFFER_SIZE];

    loop {
        match src.read(&mut buffer) {
            Ok(0) => {
                println!("[+] {} connection closed by source", direction);
                break;
            }
            Ok(bytes_read) => match dst.write_all(&buffer[..bytes_read]) {
                Ok(_) => {
                    if let Err(e) = dst.flush() {
                        println!("[!] Error flushing data in {}:{}", direction, e);
                        break;
                    }
                }
                Err(e) => {
                    println!("[!] Error writing data in {}:{}", direction, e);
                    break;
                }
            },
            Err(e) => {
                println!("[!] Error reading data in {}: {}", direction, e);
                break;
            }
        }
    }
}

fn start_proxy(
    listen_ip: &str,
    listen_port: u16,
    target_ip: String,
    target_port: u16,
) -> io::Result<()> {
    let listner = TcpListener::bind((listen_ip, listen_port))?;
    println!(
        "[+] Proxy listening on {}: {} and forwarding to {}: {}",
        listen_ip, listen_port, target_ip, target_port
    );

    for stream in listner.incoming() {
        match stream {
            Ok(client_socket) => {
                let client_addr = client_socket.peer_addr()?;
                println!("[+] Accepted connection from {}", client_addr);

                let target_ip_clone = target_ip.clone();
                thread::spawn(move || {
                    if let Err(e) = handle_client(client_socket, target_ip_clone, target_port) {
                        println!("[!] Error handling client: {}", e);
                    }
                });
            }
            Err(e) => {
                println!("[!] Error accepting connection: {}", e);
            }
        }
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() != 2 {
        eprintln!("Usage: {} <target_ip>", args[0]);
        eprintln!("Example: {} 192.168.1.100", args[0]);

        std::process::exit(1);
    }

    let target_ip = args[1].clone();
    if target_ip.parse::<std::net::IpAddr>().is_err() {
        eprintln!("[!] Invalid IP address: {}", target_ip);
        std::process::exit(1);
    }

    println!("[+] Starting TCP proxy server ...");

    if let Err(e) = start_proxy(LISTEN_IP, LISTEN_PORT, target_ip, TARGET_PORT) {
        eprintln!("[!] Proxy server error: {}", e);
        std::process::exit(1);
    }
}
