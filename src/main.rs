use std::{
    io::{self, prelude::*},
    net::{TcpListener, TcpStream},
};

use log::info;

fn main() -> io::Result<()> {
    env_logger::init();

    let listener = TcpListener::bind("127.0.0.1:4221")?;

    info!("Listening on port 4221");

    let (stream, _) = listener.accept()?;
    handle_connection(stream)?;

    Ok(())
}

fn handle_connection(mut stream: TcpStream) -> io::Result<()> {
    info!("accepted new connection");

    stream.write_all(b"HTTP/1.1 200 OK\r\n\r\n")?;

    info!("closing connection");
    Ok(())
}
