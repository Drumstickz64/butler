use std::{
    io::{self, prelude::*},
    net::{TcpListener, TcpStream},
    str::FromStr,
};

use log::{debug, info};

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

    let mut buf = vec![0; 1024];
    let bytes_read = stream.read(&mut buf)?;
    buf.truncate(bytes_read);
    buf.shrink_to_fit();

    let request = String::from_utf8(buf).unwrap();

    debug!("request is {request:?}");

    let mut parts = request.split("\r\n");

    let request_line: RequestLine = parts
        .next()
        .expect("expected request line to be present")
        .parse()
        .expect("invalid request line");

    debug!("request line: {request_line:?}");

    if request_line.url == "/" {
        stream.write_all(b"HTTP/1.1 200 OK\r\n\r\n")?;
    } else {
        stream.write_all(b"HTTP/1.1 404 Not Found\r\n\r\n")?;
    }

    info!("closing connection");
    Ok(())
}

#[derive(Debug, Clone)]
struct RequestLine {
    pub url: String,
}

impl FromStr for RequestLine {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let url = s
            .split(" ")
            .nth(1)
            .ok_or_else(|| "expected URL".to_owned())?
            .to_owned();

        Ok(Self { url })
    }
}
