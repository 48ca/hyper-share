#[macro_use]
extern crate lazy_static;

mod display;
mod http;
mod opts;
mod rendering;

use display::types::{ConnectionSet, ControlEvent};

use display::display;

use clap::Clap;

use std::{fs::canonicalize, path::Path};

use std::io;

use termion::{event::Key, input::TermRead};

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

use crate::opts::types::Opts;
use http::HttpTui;
use nix::unistd;
use std::{sync::mpsc, thread};

fn main() -> Result<(), io::Error> {
    let opts: Opts = Opts::parse();
    let path = Path::new(&opts.directory);
    let canon_path = match canonicalize(path) {
        Ok(path) => path,
        Err(e) => {
            eprintln!("Failed to open directory {}: {}", opts.directory, e);
            return Ok(());
        }
    };

    let (hist_tx, hist_rx) = mpsc::channel();

    let mut tui = match HttpTui::new(
        &opts.host,
        opts.port,
        &canon_path.as_path(),
        hist_tx,
        !opts.disable_directory_listings,
        opts.start_disabled,
        opts.uploading_enabled,
    ) {
        Ok(tui) => tui,
        Err(e) => {
            eprintln!("Failed to bind to port {}: {}", opts.port, e);
            return Ok(());
        }
    };

    let (read_end, write_end) = match unistd::pipe() {
        Ok(tuple) => tuple,
        Err(_) => {
            eprintln!("Could not create pipe :(");
            return Ok(());
        }
    };

    if !opts.headless {
        let connection_set = Arc::new(Mutex::new(ConnectionSet::new()));
        let connection_set_needs_update = Arc::new(AtomicBool::new(false));

        let needs_update_clone = Arc::clone(&connection_set_needs_update);

        let (tx, rx) = mpsc::channel();

        let connection_set_ptr = connection_set.clone();
        let canon_path = canon_path.clone();
        let thd = thread::spawn(move || {
            match display(
                canon_path.display(),
                connection_set_ptr,
                rx,
                &needs_update_clone,
                write_end,
                opts,
            ) {
                Err(e) => {
                    eprintln!("Got io::Error while displaying: {}", e);
                }
                _ => {}
            }
        });

        let keys = thread::spawn(move || {
            let stdin = io::stdin();
            for evt in stdin.keys() {
                if let Ok(key) = evt {
                    match key {
                        Key::Ctrl('c') => {
                            let _ = tx.send(ControlEvent::Quit);
                            break;
                        }
                        Key::Char('q') => {
                            let _ = tx.send(ControlEvent::Quit);
                            break;
                        }
                        Key::Char('k') => {
                            let _ = tx.send(ControlEvent::CloseAll);
                        }
                        Key::Char(' ') => {
                            let _ = tx.send(ControlEvent::Toggle);
                        }
                        _ => {}
                    }
                }
            }
        });

        tui.run(read_end, move |connections| {
            if connection_set_needs_update.load(Ordering::Acquire) {
                let mut conn_set = connection_set.lock().unwrap();
                conn_set.update(&connections);
                loop {
                    match hist_rx.try_recv() {
                        Ok(s) => {
                            conn_set.history.push(s);
                        }
                        Err(mpsc::TryRecvError::Empty) => {
                            break;
                        }
                        Err(mpsc::TryRecvError::Disconnected) => {
                            break;
                        }
                    }
                }
                connection_set_needs_update.store(false, Ordering::Release);
            }
        });

        let _ = unistd::close(read_end);

        let _ = thd.join();
        let _ = keys.join();
    } else {
        println!("Listening on {}:{}", opts.host, opts.port);
        tui.run(read_end, move |_connections| loop {
            match hist_rx.try_recv() {
                Ok(s) => {
                    println!("{}", s);
                }
                Err(mpsc::TryRecvError::Empty) | Err(mpsc::TryRecvError::Disconnected) => {
                    break;
                }
            }
        });
        let _ = unistd::close(read_end);
    }

    Ok(())
}
