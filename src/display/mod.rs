pub mod types;

use crate::opts::types::Opts;

use types::{Connection, ConnectionSet, ControlEvent};

use termion::{raw::IntoRawMode, screen::AlternateScreen};
use tui::{
    backend::TermionBackend,
    layout::{Constraint, Direction, Layout},
    text::{Span, Spans},
    widgets::{Block, Borders, List, ListItem},
    Terminal,
};

use std::{
    io,
    path::Display,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread, time,
};

use nix::unistd;
use std::{net::SocketAddr, os::unix::io::RawFd};

fn build_conn_str(addr: &SocketAddr, conn: &mut Connection) -> String {
    let ip_str = match addr {
        SocketAddr::V4(v4_addr) => {
            format!("{host}:{port}", host = v4_addr.ip(), port = v4_addr.port())
        }
        SocketAddr::V6(v6_addr) => {
            format!(
                "[{host}:{port}]",
                host = v6_addr.ip(),
                port = v6_addr.port()
            )
        }
    };

    format!(
        "{ip_req:<26} => {uri}",
        ip_req = format!("{ip:<22} #{num}", ip = ip_str, num = conn.num_requests,),
        uri = conn.last_requested_uri
    )
}

fn build_speed_str(conn: &mut Connection) -> String {
    let perc = if conn.bytes_requested == 0 {
        0
    } else {
        100 * conn.bytes_sent / conn.bytes_requested
    };
    let speed = conn.estimated_speed();
    let speed_str = format!(
        "D:{sent}/{reqd}\t ({perc}% {speed} MiB/s) U:{upsent}",
        sent = conn.bytes_sent,
        reqd = conn.bytes_requested,
        perc = perc,
        speed = speed / (1024. * 1024.),
        upsent = conn.bytes_read,
    );

    speed_str
}

fn build_conn_span<'a>(
    addr: &'a SocketAddr,
    conn: &'a mut Connection,
    term_width: u16,
) -> Vec<Spans<'static>> {
    let conn_s = build_conn_str(addr, conn);
    let speed_s = build_speed_str(conn);

    if conn_s.len() + speed_s.len() + 1 <= (term_width - 4) as usize {
        vec![Spans::from(Span::raw(format!("{} {}", conn_s, speed_s)))]
    } else {
        vec![
            Spans::from(Span::raw(conn_s)),
            Spans::from(Span::raw(format!(" >>> {}", speed_s))),
        ]
    }
}

pub fn display(
    root_path: Display,
    connection_set: Arc<Mutex<ConnectionSet>>,
    rx: mpsc::Receiver<ControlEvent>,
    needs_update: &AtomicBool,
    write_end: RawFd,
    opts: &Opts,
) -> Result<(), io::Error> {
    let stdout = io::stdout().into_raw_mode()?;
    let stdout = AlternateScreen::from(stdout);
    let backend = TermionBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut enabled = !opts.start_disabled;

    'outer: loop {
        // Print that the connection has been established
        {
            let width = terminal.size()?.width;
            let conn_set = &mut connection_set.lock().unwrap();
            let messages_connections: Vec<ListItem> = {
                conn_set
                    .connections
                    .iter_mut()
                    .map(|(addr, conn)| ListItem::new(build_conn_span(addr, conn, width)))
                    .collect()
            };

            let messages_history: Vec<ListItem> = {
                conn_set
                    .history
                    .iter()
                    .map(|s| ListItem::new(vec![Spans::from(Span::raw(s))]))
                    .collect()
            };

            terminal.draw(|f| {
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .margin(1)
                    .constraints(
                        [
                            Constraint::Length(7),
                            Constraint::Min(2),
                            Constraint::Percentage(50),
                        ]
                        .as_ref(),
                    )
                    .split(f.size());

                let block = List::new(vec![
                    ListItem::new(vec![Spans::from(Span::raw(format!(
                        "Serving {}",
                        root_path,
                    )))]),
                    ListItem::new(vec![Spans::from(Span::raw(format!(
                        "Listening on {}:{}",
                        opts.hostmask, opts.port
                    )))]),
                    ListItem::new(vec![Spans::from(Span::raw(format!(
                        "Directory listings: {}",
                        if opts.disable_directory_listings {
                            "Disabled"
                        } else {
                            "Enabled"
                        }
                    )))]),
                    ListItem::new(vec![Spans::from(Span::raw(format!(
                        "Uploading: {}{}",
                        if opts.uploading_enabled {
                            "Enabled"
                        } else {
                            "Disabled"
                        },
                        if opts.size_limit > 0 && opts.uploading_enabled {
                            format!(" (limit: {})", opts.size_limit)
                        } else {
                            format!("")
                        }
                    )))]),
                    ListItem::new(vec![Spans::from(Span::raw(format!(
                        "Status: {}",
                        if enabled {
                            "Serving requests"
                        } else {
                            "Rejecting requests"
                        },
                    )))]),
                ])
                .block(Block::default().borders(Borders::ALL).title("Information"));
                f.render_widget(block, chunks[0]);

                let block = List::new(messages_connections)
                    .block(Block::default().borders(Borders::ALL).title("Connections"));
                f.render_widget(block, chunks[1]);

                let block = List::new(messages_history).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Request History"),
                );
                f.render_widget(block, chunks[2]);
            })?;
        }

        loop {
            match rx.try_recv() {
                Ok(ControlEvent::Quit) => {
                    break 'outer;
                }
                Ok(ControlEvent::Toggle) => {
                    let _ = unistd::write(write_end, b"t");
                    enabled = !enabled;
                }
                Ok(ControlEvent::CloseAll) => {
                    let _ = unistd::write(write_end, b"k");
                }
                Err(mpsc::TryRecvError::Empty) => {
                    break;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    break 'outer;
                }
            }
        }

        // If we don't chill a little, we'll actually slow down the http server
        // because we'll be doing a ton of copies.
        thread::sleep(time::Duration::from_millis(opts.ui_refresh_rate));

        needs_update.store(true, Ordering::Release);

        // Poke `select` to give us more information.
        let _ = unistd::write(write_end, b"p");
    }

    let _ = unistd::close(write_end);

    Ok(())
}
