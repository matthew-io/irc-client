use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    fs,
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
        stream.write_all(b"Usage: REGISTER <username> <password>\r\n")?;
        return Ok(());
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
        stream.write_all(b"Username already exists.\r\n")?;
    } else {
        users.insert(username.clone(), hashed_password);
        stream.write_all(b"Registration succesful.\r\n")?;
        log::info!("User registered: {}", username);
    }
    Ok(())
}

fn login_user(
    message: &str,
    users: &SharedUsers,
    stream: &mut TcpStream,
    nickname: &mut String,
) -> Result<(), std::io::Error> {
    let parts: Vec<&str> = message.splitn(3, ' ').collect();
    if parts.len() != 3 {
        stream.write_all(b"Usage: LOGIN <username> <password>\r\n")?;
        return Ok(());
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
            stream.write_all(b"Login successful.\r\n")?;
            log::info!("User logged in: {}", username);
        } else {
            stream.write_all(b"Invalid password.\r\n")?;
        }
    } else {
        stream.write_all(b"User not found.\r\n")?;
    }
    Ok(())
}


fn set_nickname(
    nickname: &mut String,
    message: &str,
    clients: &SharedClients,
    stream: &mut TcpStream,
) -> Result<(), std::io::Error> {
    let parts: Vec<&str> = message.splitn(2, ' ').collect();
    if parts.len() == 2 {
        *nickname = parts[1].to_string();
        clients
            .lock()
            .unwrap()
            .insert(nickname.clone(), Arc::new(Mutex::new(stream.try_clone()?)));
        stream.write_all(format!("Nickname set to {}\r\n", nickname).as_bytes())?;
    } else {
        stream.write_all(b"Invalid NICK command.\r\n")?;
    }
    Ok(())
}

fn join_channel(
    nickname: &str,
    message: &str,
    channels: &SharedChannels,
    stream: &mut TcpStream,
) -> Result<(), std::io::Error> {
    let parts: Vec<&str> = message.splitn(2, ' ').collect();
    if parts.len() != 2 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Invalid JOIN command format.",
        ));
    }

    let channel_name = parts[1].to_string();
    let mut channels = channels.lock().map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::Other, "Failed to lock channels")
    })?;
    let channel = channels.entry(channel_name.clone()).or_insert(Vec::new());

    if !channel.contains(&nickname.to_string()) {
        channel.push(nickname.to_string());
        stream.write_all(format!("Joined channel {}\r\n", channel_name).as_bytes())?;
        log::info!("{} joined channel {}", nickname, channel_name);
    }
    Ok(())
}


fn leave_channel(
    nickname: &str,
    message: &str,
    channels: &SharedChannels,
    stream: &mut TcpStream,
) -> Result<(), std::io::Error> {
    let parts: Vec<&str> = message.splitn(2, ' ').collect();
    if parts.len() == 2 {
        let channel_name = parts[1].to_string();
        let mut channels = channels.lock().map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::Other, "Failed to lock channels")
        })?;
        if let Some(channel) = channels.get_mut(&channel_name) {
            channel.retain(|name| name != nickname);
            if channel.is_empty() {
                channels.remove(&channel_name);
            }
            stream.write_all(format!("Left channel {}\r\n", channel_name).as_bytes())?;
        } else {
            stream.write_all(b"Channel not found.\r\n")?;
        }
    } else {
        stream.write_all(b"Invalid PART command.\r\n")?;
    }
    Ok(())
}

fn remove_client(
    nickname: &str,
    clients: &SharedClients,
    channels: &SharedChannels,
) {
    clients.lock().unwrap().remove(nickname);
    let mut channels = channels.lock().unwrap();
    for channel in channels.values_mut() {
        channel.retain(|name| name != nickname);
    }
}

fn send_message (
    nickname: &str,
    message: &str,
    clients: &SharedClients,
    channels: &SharedChannels,
    stream: &mut TcpStream,
) -> Result<(), std::io::Error> {
    let parts: Vec<&str> = message.splitn(3, ' ').collect();
    if parts.len() == 3 {
        let target = parts[1];
        let msg = parts[2];
        let clients = clients.lock().unwrap();
        let channels = channels.lock().unwrap();

        if target.starts_with('#') {
            if let Some(channel) = channels.get(target) {
                for name in channel {
                    if let Some(mut client) = clients.get(name) {
                        let full_message = format!("[{}] {}: {}\r\n", target, nickname, msg);
                        client.write_all(full_message.as_bytes())?;
                    }
                }
            } else {
                stream.write_all(b"Channel not found.\r\n")?;
            }
        } else if let Some(mut client) = clients.get(target) {
            let private_message = format!("{}: {}\r\n", nickname, msg);
            client.write_all(private_message.as_bytes());
        } else {
            stream.write_all(b"User not found.\r\n")?;
        }
    } else {
        stream.write_all(b"Invalid PRIVMSG command.\r\n")?;
    }
    Ok(())
} 

fn handle_client(
    mut stream: TcpStream,
    clients: SharedClients,
    channels: SharedChannels,
    users: SharedUsers,
) {
    let mut nickname = String::new();
    if let Err(e) = stream.write_all(b"Welcome to the IRC server! Use REGISTER or LOGIN.\r\n") {
        log::error!("Failed to send welcome message: {}", e);
        return;
    }

    let mut buffer = [0; 512];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => {
                log::info!("Client disconnected: {}", nickname);
                break;
            }
            Ok(bytes) => {
                let message = String::from_utf8_lossy(&buffer[..bytes]).trim().to_string();
                log::debug!("Received: {}", message);

                let result = if message.starts_with("REGISTER") {
                    register_user(&message, &users, &mut stream)
                } else if message.starts_with("LOGIN") {
                    login_user(&message, &users, &mut stream, &mut nickname)
                } else if !nickname.is_empty() {
                    if message.starts_with("NICK") {
                        set_nickname(&mut nickname, &message, &clients, &mut stream)
                    } else if message.starts_with("JOIN") {
                        join_channel(&nickname, &message, &channels, &mut stream)
                    } else if message.starts_with("PART") {
                        leave_channel(&nickname, &message, &channels, &mut stream)
                    } else if message.starts_with("PRIVMSG") {
                        send_message(&nickname, &message, &clients, &channels, &mut stream)
                    } else {
                        stream.write_all(b"Unknown command.\r\n")
                    }
                } else {
                    stream.write_all(b"Please log in first.\r\n")
                };

                if let Err(e) = result {
                    log::error!("Error handling message: {}", e);
                }
            }
            Err(e) => {
                log::error!("Error reading from client: {}", e);
                break;
            }
        }
    }

    remove_client(&nickname, &clients, &channels);
}

fn main() {
    env_logger::init();

    let address = "127.0.0.1:6667";
    let listener = TcpListener::bind(address).expect("Failed to bind to address");

    let clients: SharedClients = Arc::new(Mutex::new(HashMap::new()));
    let channels: SharedChannels = Arc::new(Mutex::new(HashMap::new()));
    let users = load_users(); // Load users from file

    log::info!("Server is running on {}", address);

    let result = std::panic::catch_unwind(|| {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    log::info!("New client connected!");

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
    });

    if let Err(_) = save_users(&users) {
        log::error!("Failed to save user database.");
    }

    if let Err(e) = result {
        log::error!("Server crashed: {:?}", e);
    }
}