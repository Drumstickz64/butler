#![warn(rust_2018_idioms)]
#![warn(missing_debug_implementations)]

use std::{
    fmt,
    fs::{self, File},
    io::{self, prelude::*},
    net::{TcpListener, TcpStream},
    str::FromStr,
};

use anyhow::{anyhow, Context};

use threadpool::ThreadPool;

use flate2::{write::GzEncoder, Compression};

const NUM_THREADS: usize = 500;

type ConnId = usize;

fn main() -> anyhow::Result<()> {
    env_logger::init();

    let pool = ThreadPool::new(NUM_THREADS);
    let listener = TcpListener::bind("127.0.0.1:4221")?;

    log::info!("Listening on port 4221");

    for (conn_id, stream) in listener.incoming().enumerate() {
        match stream {
            Ok(stream) => {
                pool.execute(move || {
                    if let Err(err) = handle_connection(stream, conn_id) {
                        log::error!("error while handling connection: {err}");
                    }
                });
            }
            Err(err) => log::error!("error while attempting to establish a connection: {err}"),
        };
    }

    Ok(())
}

fn handle_connection(mut stream: TcpStream, id: ConnId) -> anyhow::Result<()> {
    log::info!("accepted connection {id}");

    let mut buf = [0; 4096];
    let bytes_read = stream
        .read(&mut buf)
        .context("failed to read from client")?;

    let request = String::from_utf8_lossy(&buf);

    log::debug!("id = {id}, request string = {}", &request[..bytes_read]);

    let request: Request = request[..bytes_read]
        .parse()
        .context("failed to parse request")?;

    log::debug!("id = {id}, request = {request:#?}");

    let mut response = match request.line.url.as_ref() {
        "/" => Response::empty(),
        "/user-agent" => {
            let user_agent = request
                .headers
                .iter()
                .find_map(|header| {
                    if let Header::UserAgent(agent) = header {
                        Some(agent)
                    } else {
                        None
                    }
                })
                .context("request does not have a 'User-Agent' header")?;

            Response::text(user_agent.to_owned())
        }
        url => {
            if let Some(string) = url.strip_prefix("/echo/") {
                Response::text(string.to_owned())
            } else if let Some(file_name) = url.strip_prefix("/files/") {
                match request.line.method {
                    Method::Get => Response::file(file_name),
                    Method::Post => {
                        let contents = request
                            .body
                            .context("POST request to /files must have a body")?;
                        fs::write(format!("files/{file_name}"), contents)
                            .context("failed to write file to disk")?;
                        Response::created()
                    }
                }
            } else {
                Response::not_found()
            }
        }
    };

    if request.headers.contains(&Header::AcceptEncoding) {
        response = response.compressed();
    }

    log::debug!("id = {id}, response = {response:#?}");

    response
        .write_to(&mut stream)
        .context("failed to write to client")?;

    stream.flush().context("failed to write to client")?;

    log::info!("closing connection {id}");
    Ok(())
}

#[derive(Debug, Clone)]
struct Request {
    line: RequestLine,
    headers: Vec<Header>,
    body: Option<String>,
}

impl FromStr for Request {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.split_terminator("\r\n");

        let line: RequestLine = parts
            .next()
            .with_context(|| anyhow!("did not find request line in request {s}"))?
            .parse()
            .context("failed to parse request line")?;

        let headers: Vec<Header> = parts
            .by_ref()
            .take_while(|header_str| !header_str.is_empty())
            .filter_map(|header_str| {
                header_str
                    .parse()
                    .inspect_err(|err| {
                        log::warn!("failed to parse HTTP header, skipping...: {err}")
                    })
                    .ok()
            })
            .collect();

        let body = parts.next().map(String::from);

        Ok(Self {
            line,
            headers,
            body,
        })
    }
}

#[derive(Debug, Clone)]
struct RequestLine {
    method: Method,
    url: String,
}

impl FromStr for RequestLine {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.split_whitespace();

        let method: Method = parts
            .next()
            .context("could not find HTTP method in request line")?
            .parse()
            .context("failed to parse HTTP method")?;

        let url = parts
            .next()
            .context("could find URL in request line")?
            .to_owned();

        Ok(Self { method, url })
    }
}

#[derive(Debug, Clone)]
struct Response {
    // TODO: change to enum
    status_code: u32,
    headers: Vec<Header>,
    body: Option<Vec<u8>>,
}

impl Response {
    fn empty() -> Self {
        Self {
            status_code: 200,
            headers: Vec::new(),
            body: None,
        }
    }

    fn not_found() -> Self {
        Self {
            status_code: 404,
            headers: Vec::new(),
            body: None,
        }
    }

    fn text(text: String) -> Self {
        Self {
            status_code: 200,
            headers: vec![
                Header::ContentType(ContentType::TextPlain),
                Header::ContentLength(text.len()),
            ],
            body: Some(text.into_bytes()),
        }
    }

    fn created() -> Self {
        Self {
            status_code: 201,
            headers: Vec::new(),
            body: None,
        }
    }

    fn file(file_name: &str) -> Self {
        let Ok(mut file) = File::open(format!("files/{file_name}")) else {
            return Self::not_found();
        };

        // TODO: change to server error
        let mut content = Vec::new();
        if file.read_to_end(&mut content).is_err() {
            log::error!("failed to read file {file_name:?}");
            return Self::not_found();
        }

        Response {
            status_code: 200,
            headers: vec![
                Header::ContentType(ContentType::ApplicationOctetStream),
                Header::ContentLength(content.len()),
            ],
            body: Some(content),
        }
    }

    fn compressed(mut self) -> Self {
        debug_assert!(!self.headers.contains(&Header::ContentEncoding));

        self.headers.push(Header::ContentEncoding);

        if let Some(body) = self.body.as_mut() {
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(body).unwrap();
            *body = encoder.finish().unwrap();
            let content_len_header = self
                .headers
                .iter_mut()
                .find(|header| matches!(header, Header::ContentLength(_)))
                .expect("expected to have 'Content-Length' header in response with body");

            *content_len_header = Header::ContentLength(body.len());
        }

        self
    }

    fn write_to(&self, mut w: impl io::Write) -> io::Result<()> {
        write!(
            w,
            "HTTP/1.1 {status}\r\n{headers}\r\n",
            status = match self.status_code {
                200 => "200 OK",
                201 => "201 Created",
                404 => "404 Not Found",
                code => todo!("unhandled status code: {code}"),
            },
            headers = self
                .headers
                .iter()
                .map(|header| format!("{header}\r\n"))
                .fold(String::new(), |acc, s| acc + &s),
        )?;

        if let Some(body) = self.body.as_ref() {
            w.write_all(body)?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Method {
    Get,
    Post,
}

impl FromStr for Method {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_ref() {
            "get" => Ok(Self::Get),
            "post" => Ok(Self::Post),
            _ => Err(anyhow!("{s} is not a valid HTTP method")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Header {
    ContentType(ContentType),
    ContentLength(usize),
    UserAgent(String),
    // assume gzip
    AcceptEncoding,
    ContentEncoding,
}

impl fmt::Display for Header {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ContentType(content_type) => write!(f, "Content-Type: {content_type}"),
            Self::ContentLength(length) => write!(f, "Content-Length: {length}"),
            Self::ContentEncoding => write!(f, "Content-Encoding: gzip"),
            _ => todo!(),
        }
    }
}

impl FromStr for Header {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.split(':').map(str::trim);

        let name = parts
            .next()
            .context("failed to find header name, maybe it's missing a ':'?")?;

        let value = parts
            .next()
            .context("failed to find header value, maybe it's missing a ':'?")?;

        match name.to_lowercase().as_ref() {
            "user-agent" => Ok(Self::UserAgent(value.to_owned())),
            "accept-encoding" if value == "gzip" => Ok(Self::AcceptEncoding),
            "accept-encoding" => Err(anyhow!("failed to parse 'Accept-Encoding': unknown encoding {value:?}, only 'gzip' is supported")),
            name => Err(anyhow!("unknown header: {name:?}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ContentType {
    #[default]
    TextPlain,
    ApplicationOctetStream,
}

impl fmt::Display for ContentType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ContentType::TextPlain => f.write_str("text/plain"),
            ContentType::ApplicationOctetStream => f.write_str("application/octet-stream"),
        }
    }
}
