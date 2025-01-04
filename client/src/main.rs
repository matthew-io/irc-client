use std::{
    io::{self, BufRead, Read, Write},
    net::TcpStream,
};

fn main() {
    let server = "127.0.0.1:6667";
    match TcpStream::connect(server) {
        Ok(mut stream) => {
            println!("Connected to the server!");

            let stdin = io::stdin();
            let mut buffer = [0; 512];

            let mut read_stream = stream.try_clone().expect("Failed to clone stream");
            std::thread::spawn(move || {
                loop {
                    match read_stream.read(&mut buffer) {
                        Ok(0) => {
                            println!("Connection closed by server.");
                            break;
                        }
                        Ok(bytes) => {
                            println!("{}", String::from_utf8_lossy(&buffer[..bytes]));
                        }
                        Err(e) => {
                            eprintln!("Error reading from server: {}", e);
                            break;
                        }
                    }
                }
            });

            println!("Enter IRC commands (e.g., NICK <name>, PRIVMSG <target> <message>, QUIT)");
            for line in stdin.lock().lines() {
                let line = line.unwrap();
                if line == "QUIT" {
                    stream.write_all(b"Quit\r\n").unwrap();
                    println!("Goodbye!");
                    break;
                }
                stream.write_all(format!("{}\r\n", line).as_bytes()).unwrap();
            }
        }
        Err(e) => eprintln!("Failed to connect: {}", e),
    }
}