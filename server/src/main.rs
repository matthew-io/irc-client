use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread,
};

type SharedClients = Arc<Mutex<HashMap<String, TcpStream>>>;

fn handle_client(mut stream: TcpStream, clients: SharedClients) {
    let mut nickname = String::new();

    let welcome_message = "Welcome to the IRC server! Please set your nickname using NICK <name>.\r\n";
    if let Err(e) = stream.write_all(welcome_message.as_bytes()) {
        eprintln!("Failed to send welcome message: {}", e);
        return;
    }

    let mut buffer = [0; 512];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => {
                println!("Client disconnected: {}", nickname);
                break;
            }
            Ok(bytes) => {
                let message = String::from_utf8_lossy(&buffer[..bytes]).trim().to_string();
                println!("Received: {}", message);

                if message.starts_with("NICK") {
                    let parts: Vec<&str> = message.splitn(2, ' ').collect();
                    if parts.len() == 2 {
                        nickname = parts[1].to_string();
                        clients.lock().unwrap().insert(nickname.clone(), stream.try_clone().unwrap());
                        let confirmation = format!("Nickname set to {}\r\n", nickname);
                        stream.write_all(confirmation.as_bytes()).unwrap();
                    } else {
                        stream.write_all(b"Invalid NICK command.\r\n").unwrap();
                    }
                } else if message.starts_with("QUIT") {
                    println!("Client requested to quit: {}", nickname);
                    clients.lock().unwrap().remove(&nickname);
                    break;
                } else if message.starts_with("PRIVMSG") {
                    let parts: Vec<&str> = message.splitn(3, ' ').collect();
                    if parts.len() == 3 {
                        let target = parts[1];
                        let msg = parts[2];
                        let clients = clients.lock().unwrap();
                        if let Some(mut target_stream) = clients.get(target) {
                            let private_message = format!("{}: {}\r\n", nickname, msg);
                            target_stream.write_all(private_message.as_bytes()).unwrap();
                        } else {
                            stream.write_all(b"User not found.\r\n").unwrap();
                        }
                    } else {
                        stream.write_all(b"Invalid PRIVMSG command.\r\n").unwrap();
                    }
                } else {
                    stream.write_all(b"Unknown command.\r\n").unwrap();
                }
            }
            Err(e) => {
                eprintln!("Error reading from client: {}", e);
                break;
            }
        }
    }

    clients.lock().unwrap().remove(&nickname);
}

fn main() {
    let address = "127.0.0.1:6667";
    let listener = TcpListener::bind(address).expect("Failed to bind to address");
    let clients: SharedClients = Arc::new(Mutex::new(HashMap::new()));

    println!("Server is running on {}", address);

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                println!("New client connected!");
                let clients = Arc::clone(&clients);
                thread::spawn(|| handle_client(stream, clients));
            }
            Err(e) => eprintln!("Failed to accept connection: {}", e),
        }
    }
}
