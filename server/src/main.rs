use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    fs,
    time::Duration,
    thread,
};
use bcrypt::{hash, DEFAULT_COST, verify};
use serde_json;
use serde::{Serialize, Deserialize};

type SharedClients = Arc<Mutex<HashMap<String, Arc<Mutex<TcpStream>>>>>;
type SharedChannels = Arc<Mutex<HashMap<String, Vec<String>>>>;
type SharedUsers = Arc<Mutex<HashMap<String, String>>>;

fn save_users(users: &SharedUsers) -> Result<(), std::io::Error> {
    let users = users.lock().map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::Other, "Failed to lock user database")
    })?;

    let serialized = serde_json::to_string(&*users).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::Other, "Failed to serialize user database")
    })?;

    fs::write("users.json", serialized)?;
    log::info!("User database saved successfully.");
    Ok(())
}

fn load_users() -> SharedUsers {
    match fs::read_to_string("users.json") {
        Ok(data) => {
            let users: HashMap<String, String> = serde_json::from_str(&data).unwrap_or_default();
            log::info!("User database loaded.");
            Arc::new(Mutex::new(users))
        }
        Err(_) => {
            log::warn!("No user database found. Starting with an empty database.");
            Arc::new(Mutex::new(HashMap::new()))
        }
    }
}

fn register_user(
    message: &str,
    users: &SharedUsers,
    stream: &mut TcpStream,
) -> Result<(), std::io::Error> {
    let parts: Vec<&str> = message.splitn(3, ' ').collect();
    if parts.len() != 3 {
        return stream.write_all(b"Usage: REGISTER <username> <password>\r\n").map(|_| ());
    }

    let username = parts[1].to_string();
    let password = parts[2].to_string();

    let hashed_password = hash(password, DEFAULT_COST).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::Other, "Failed to hash password")
    })?;

    let mut users = users.lock().map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::Other, "Failed to lock user database")
    })?;

    if users.contains_key(&username) {
        stream.write_all(b"Username already exists.\r\n").map(|_| ())
    } else {
        users.insert(username.clone(), hashed_password);
        stream.write_all(b"Registration successful.\r\n")?;
        log::info!("User registered: {}", username);
        Ok(())
    }
}

fn login_user(
    message: &str,
    users: &SharedUsers,
    clients: &SharedClients,
    stream: &mut TcpStream,
    nickname: &mut String,
) -> Result<(), std::io::Error> {
    let parts: Vec<&str> = message.splitn(3, ' ').collect();
    if parts.len() != 3 {
        return stream.write_all(b"Usage: LOGIN <username> <password>\r\n").map(|_| ());
    }

    let username = parts[1].to_string();
    let password = parts[2].to_string();

    let users = users.lock().map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::Other, "Failed to lock user database")
    })?;

    if let Some(hashed_password) = users.get(&username) {
        if verify(password, hashed_password).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::Other, "Failed to verify password")
        })? {
            *nickname = username.clone();

            let mut clients = clients.lock().unwrap();
            clients.insert(username.clone(), Arc::new(Mutex::new(stream.try_clone()?)));

            stream.write_all(b"Login successful.\r\n")?;
            log::info!("User logged in: {}", username);
            Ok(())
        } else {
            stream.write_all(b"Invalid password.\r\n").map(|_| ())
        }
    } else {
        stream.write_all(b"User not found.\r\n").map(|_| ())
    }
}

fn handle_stats(
    clients: &SharedClients,
    channels: &SharedChannels,
    stream: &mut TcpStream,
) -> Result<(), std::io::Error> {
    let clients_count = clients.lock().unwrap().len();
    let channels_count = channels.lock().unwrap().len();
    let stats = format!(
        "Active clients: {}\nActive channels: {}\r\n",
        clients_count, channels_count
    );
    stream.write_all(stats.as_bytes())?;
    Ok(())
}

fn join_channel(
    nickname: &str,
    channel_name: &str,
    channels: &SharedChannels,
    stream: &mut TcpStream,
) -> Result<(), std::io::Error> {
    let mut channels = channels.lock().unwrap();
    let channel = channels.entry(channel_name.to_string()).or_insert(Vec::new());

    if !channel.contains(&nickname.to_string()) {
        channel.push(nickname.to_string());
        let msg = format!("Joined channel {}\r\n", channel_name);
        stream.write_all(msg.as_bytes())?;
        log::info!("{} joined channel {}", nickname, channel_name);
    }
    Ok(())
}

fn send_message(
    nickname: &str,
    target: &str,
    msg: &str,
    clients: &SharedClients,
    channels: &SharedChannels,
    stream: &mut TcpStream,
) -> Result<(), std::io::Error> {
    let clients = clients.lock().unwrap();
    let channels = channels.lock().unwrap();

    if target.starts_with('#') {
        if let Some(channel) = channels.get(target) {
            for name in channel {
                if let Some(client) = clients.get(name) {
                    let mut client = client.lock().unwrap();
                    let full_message = format!("[{}] {}: {}\r\n", target, nickname, msg);
                    client.write_all(full_message.as_bytes())?;
                }
            }
        } else {
            stream.write_all(b"Channel not found.\r\n")?;
        }
    } else {
        if let Some(client) = clients.get(target) {
            let mut client = client.lock().unwrap();
            let private_message = format!("(Private) {}: {}\r\n", nickname, msg);
            client.write_all(private_message.as_bytes())?;
        } else {
            stream.write_all(b"User not found.\r\n")?;
        }
    }
    Ok(())
}

fn set_nickname(
    nickname: &mut String,
    new_nickname: &str,
    clients: &SharedClients,
    stream: &mut TcpStream,
) -> Result<(), std::io::Error> {
    let mut clients_map = clients.lock().unwrap();
    if clients_map.contains_key(new_nickname) {
        stream.write_all(b"Nickname already in use.\r\n").map(|_| ())
    } else {
        *nickname = new_nickname.to_string();
        clients_map.insert(new_nickname.to_string(), Arc::new(Mutex::new(stream.try_clone()?)));
        let resp = format!("Nickname set to {}\r\n", new_nickname);
        stream.write_all(resp.as_bytes())?;
        log::info!("Client set nickname to {}", new_nickname);
        Ok(())
    }
}

fn parse_command(
    message: &str,
    nickname: &mut String,
    stream: &mut TcpStream,
    clients: &SharedClients,
    channels: &SharedChannels,
    users: &SharedUsers,
) -> Result<(), std::io::Error> {
    let is_logged_in = !nickname.is_empty();

    let trimmed = message.trim_start_matches('/');
    let mut parts = trimmed.splitn(2, ' ');
    let command_part = parts.next().unwrap_or("").to_uppercase();
    let rest = parts.next().unwrap_or("").trim();

    match command_part.as_str() {
        "REGISTER" => register_user(message, users, stream),
        "LOGIN" => login_user(message, users, clients, stream, nickname),
        "NICK" => {
            if !is_logged_in {
                stream.write_all(b"Please log in first.\r\n")?;
                return Ok(());
            }
            let cmd_parts: Vec<&str> = message.splitn(2, ' ').collect();
            if cmd_parts.len() != 2 {
                return stream.write_all(b"Usage: NICK <name>\r\n").map(|_| ());
            }
            set_nickname(nickname, cmd_parts[1], clients, stream)
        }
        "STATS" => {
            if !is_logged_in {
                stream.write_all(b"Please log in first.\r\n")?;
                return Ok(());
            }
            handle_stats(clients, channels, stream)
        }
        "JOIN" => {
            if !is_logged_in {
                stream.write_all(b"Please log in first.\r\n")?;
                return Ok(());
            }
            if rest.is_empty() {
                stream.write_all(b"Usage: JOIN #channel\r\n")?;
                return Ok(());
            }
            join_channel(nickname, rest, channels, stream)
        }
        "MSG" => {
            if !is_logged_in {
                stream.write_all(b"Please log in first.\r\n")?;
                return Ok(());
            }

            let cmd_parts: Vec<&str> = trimmed.splitn(3, ' ').collect();
           
            if cmd_parts.len() != 3 {
                return stream.write_all(b"Usage: /msg <target> <message>\r\n").map(|_| ());
            }
            let target = cmd_parts[1];
            let msg = cmd_parts[2];
            send_message(nickname, target, msg, clients, channels, stream)
        }
        "PRIVMSG" => {
            if !is_logged_in {
                stream.write_all(b"Please log in first.\r\n")?;
                return Ok(());
            }
            let cmd_parts: Vec<&str> = trimmed.splitn(3, ' ').collect();
            if cmd_parts.len() != 3 {
                return stream.write_all(b"Usage: PRIVMSG <target> <message>\r\n").map(|_| ());
            }
            let target = cmd_parts[1];
            let msg = cmd_parts[2];
            send_message(nickname, target, msg, clients, channels, stream)
        }
        _ => {
            if !is_logged_in {
                stream.write_all(b"Please log in first.\r\n")?;
            } else {
                stream.write_all(b"Unknown command.\r\n")?;
            }
            Ok(())
        }
    }
}

fn handle_client(
    mut stream: TcpStream,
    clients: SharedClients,
    channels: SharedChannels,
    users: SharedUsers,
) {
    let mut nickname = String::new();
    let mut buffer = [0; 512];

    if let Err(e) = stream.write_all(b"Welcome to the IRC server! Use REGISTER <u> <p> or LOGIN <u> <p>.\r\n") {
        log::error!("Failed to send welcome message: {}", e);
        return;
    }

    loop {
        match stream.read(&mut buffer) {
            Ok(0) => {
                log::info!("Client disconnected: {}", nickname);
                break;
            }
            Ok(bytes) => {
                let message = String::from_utf8_lossy(&buffer[..bytes]).trim().to_string();
                if message.is_empty() {
                    continue; 
                }

                let result = parse_command(
                    &message,
                    &mut nickname,
                    &mut stream,
                    &clients,
                    &channels,
                    &users,
                );
                if let Err(e) = result {
                    log::error!("Error handling message: {}", e);
                }
            }
            Err(e) => {
                log::error!("Error reading from client: {}", e);
                break;
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let address = "127.0.0.1:6667";
    let listener = TcpListener::bind(address).expect("Failed to bind to address");

    let clients: SharedClients = Arc::new(Mutex::new(HashMap::new()));
    let channels: SharedChannels = Arc::new(Mutex::new(HashMap::new()));
    let users = load_users();

    log::info!("Server is running on {}", address);

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let clients = Arc::clone(&clients);
                let channels = Arc::clone(&channels);
                let users = Arc::clone(&users);

                thread::spawn(move || {
                    handle_client(stream, clients, channels, users);
                });
            }
            Err(e) => log::error!("Failed to accept connection: {}", e),
        }
    }
}
