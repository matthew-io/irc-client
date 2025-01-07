use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    net::TcpStream,
    sync::{Arc, Mutex},
    thread,
};

use gtk::{
    prelude::*,
    Application,
    ApplicationWindow,
    Box,
    Button,
    Entry,
    Notebook,
    ScrolledWindow,
    TextView,
    Orientation,
};

pub fn run_gui() {
    let app = Application::new(Some("com.example.irc-client"), Default::default());

    app.connect_activate(move |app| {
        let server_address = "127.0.0.1:6667";

        match TcpStream::connect(server_address) {
            Ok(stream) => {
                println!("Connected to IRC server at {}", server_address);
                let stream = Arc::new(Mutex::new(stream));
                build_ui(app, stream);
            }
            Err(e) => {
                eprintln!("Failed to connect to server: {}", e);
            }
        }
    });

    app.run();
}

fn handle_send_command(
    entry: &gtk::Entry,
    stream: &Arc<Mutex<TcpStream>>,
    notebook: &gtk::Notebook,
    text_views: &Arc<Mutex<HashMap<String, TextView>>>,
) {
    let text = entry.text().to_string();
    if text.is_empty() {
        return;
    }

    if let Some(channel_name) = text.strip_prefix("/join ") {
        let normalized_name = channel_name.trim();
        if !text_views.lock().unwrap().contains_key(normalized_name) {
            let (new_tab, new_text_view) = create_channel_tab(normalized_name);
            text_views
                .lock()
                .unwrap()
                .insert(normalized_name.to_string(), new_text_view);
            notebook.append_page(&new_tab, Some(&gtk::Label::new(Some(normalized_name))));
            notebook.show_all();
        }
    }

    let mut s = stream.lock().unwrap();
    if let Err(e) = writeln!(s, "{}", text) {
        eprintln!("Failed to send command: {}", e);
    }
    if let Err(e) = s.flush() {
        eprintln!("Failed to flush stream: {}", e);
    }

    entry.set_text("");
}

fn create_channel_tab(channel_name: &str) -> (gtk::Box, gtk::TextView) {
    let vbox = gtk::Box::new(Orientation::Vertical, 5);
    let text_view = TextView::new();
    text_view.set_editable(false);
    text_view.set_cursor_visible(false);

    let scrolled_window = ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    scrolled_window.add(&text_view);

    vbox.pack_start(&scrolled_window, true, true, 0);

    (vbox, text_view)
}

fn build_ui(app: &Application, stream: Arc<Mutex<TcpStream>>) {
    let window = ApplicationWindow::new(app);
    window.set_title("IRC Client");
    window.set_default_size(800, 600);

    let vbox = Box::new(Orientation::Vertical, 5);

    let notebook = Notebook::new();
    notebook.set_tab_pos(gtk::PositionType::Top);
    notebook.set_hexpand(true);
    notebook.set_vexpand(true);

    let text_views: Arc<Mutex<HashMap<String, TextView>>> = Arc::new(Mutex::new(HashMap::new()));

    let (general_tab, general_text_view) = create_channel_tab("General");
    text_views
        .lock()
        .unwrap()
        .insert("General".to_string(), general_text_view);
    notebook.append_page(&general_tab, Some(&gtk::Label::new(Some("General"))));

    vbox.pack_start(&notebook, true, true, 0);

    let entry = Entry::new();
    entry.set_placeholder_text(Some("Type /join #channel or another command..."));
    let send_button = Button::with_label("Send");

    {
        let stream = Arc::clone(&stream);
        let notebook = notebook.clone();
        let text_views = Arc::clone(&text_views);
        let entry_clone = entry.clone();

        send_button.connect_clicked(move |_| {
            handle_send_command(&entry_clone, &stream, &notebook, &text_views);
        });
    }
    {
        let stream = Arc::clone(&stream);
        let notebook = notebook.clone();
        let text_views = Arc::clone(&text_views);
        let entry_clone = entry.clone();

        entry.connect_activate(move |_| {
            handle_send_command(&entry_clone, &stream, &notebook, &text_views);
        });
    }

    vbox.pack_start(&entry, false, false, 0);
    vbox.pack_start(&send_button, false, false, 0);

    window.add(&vbox);

    let (sender, receiver) = glib::MainContext::channel(glib::PRIORITY_DEFAULT);
    let stream_clone = Arc::clone(&stream);

    thread::spawn(move || {
        let mut reader = BufReader::new(stream_clone.lock().unwrap().try_clone().unwrap());
        let mut response = String::new();

        while let Ok(_) = reader.read_line(&mut response) {
            if response.is_empty() {
                break; 
            }
            if sender.send(response.clone()).is_err() {
                eprintln!("Failed to send response to main thread.");
                break;
            }
            response.clear();
        }
    });

    receiver.attach(None, move |raw_line: String| {
        println!("(debug) Received raw line: {}", raw_line.trim_end());

        if raw_line.starts_with("[#") {
            if let Some(end_bracket) = raw_line.find(']') {
                let channel_name = &raw_line[1..end_bracket];
                let after_bracket = &raw_line[end_bracket+2..]; 

                if let Some(text_view) = text_views.lock().unwrap().get(channel_name) {
                    let buffer = text_view.buffer().unwrap();
                    buffer.insert(&mut buffer.end_iter(), &format!("{}\n", after_bracket));
                } else {
                    if let Some(general) = text_views.lock().unwrap().get("General") {
                        let buffer = general.buffer().unwrap();
                        buffer.insert(
                            &mut buffer.end_iter(),
                            &format!("(No tab for {}): {}\n", channel_name, after_bracket),
                        );
                    }
                }
            }
        }
        else {
            let line = raw_line.trim_start_matches(':'); 
            let parts: Vec<&str> = line.splitn(4, ' ').collect();

            if parts.len() >= 4 && parts[1] == "PRIVMSG" {
                let target = parts[2];
                let message = parts[3].trim_start_matches(':');

                if let Some(text_view) = text_views.lock().unwrap().get(target) {
                    let buffer = text_view.buffer().unwrap();
                    buffer.insert(&mut buffer.end_iter(), &format!("{}: {}\n", target, message));
                } else {
                    if let Some(general) = text_views.lock().unwrap().get("General") {
                        let buffer = general.buffer().unwrap();
                        buffer.insert(
                            &mut buffer.end_iter(),
                            &format!("(No tab for {}): {}\n", target, message),
                        );
                    }
                }
            } else {
                if let Some(general) = text_views.lock().unwrap().get("General") {
                    let buffer = general.buffer().unwrap();
                    buffer.insert(&mut buffer.end_iter(), &raw_line);
                }
            }
        }

        glib::Continue(true)
    });

    window.show_all();
}
