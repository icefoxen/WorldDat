use std::fs::File;
use std::io::{Read, Write};
use std::result;
use std::str::{self, FromStr};
use std::time::{Duration, Instant};

use failure::{Error, ResultExt};
use futures::{Future, Stream};
use quicr;
use rmp_serde;
use tokio;
use tokio::runtime::current_thread;
use tokio::runtime::current_thread::Runtime;

#[derive(Clone, Debug, Serialize, Deserialize)]
enum Message {
    Ping { id: u32 },
    Pong { id: u32 },
}

use {PeerOpt, ServerOpt};
type Result<T> = result::Result<T, Error>;

fn duration_secs(x: &Duration) -> f32 {
    x.as_secs() as f32 + x.subsec_nanos() as f32 * 1e-9
}

fn run_ping(stream: quicr::Stream) -> impl Future<Item = (), Error = ()> {
    info!("Trying to send ping");
    let message = Message::Ping { id: 999 };
    let serialized_message = rmp_serde::to_vec(&message).expect("Could not serialize message?!");
    tokio::io::write_all(stream, serialized_message)
        .map_err(|e| warn!("Failed to send request: {}", e))
        /*
        .and_then(|(stream, _v)| {
            tokio::io::shutdown(stream)
                .map_err(|e| warn!("Failed to shut down stream: {}", e))
        })
*/
        .and_then(move |(mut stream, _v)| {
            stream.flush().unwrap();
            info!("Sent, reading?");
            quicr::read_to_end(stream, 1024*64)
                .map_err(|e| warn!("failed to read response: {}", e))
        })
        .and_then(move |(stream, req)| {
            let msg: ::std::result::Result<Message, rmp_serde::decode::Error> = rmp_serde::from_slice(&req);
            debug!("Got response: {:?}", msg);
            let to_send = match msg {
                Ok(Message::Ping{id}) => {
                    info!("Trying to send pong");
                    let message = Message::Pong{id};
                    rmp_serde::to_vec(&message).unwrap()
                },
                Ok(val) => {
                    info!("Got message: {:?}, not doing anything with it", val);
                    vec![]
                },
                Err(e) => {
                    info!("Got unknown message: {:?}, error {:?}", &req, e);
                    vec![]
                }
            };

            //::std::thread::sleep(::std::time::Duration::from_millis(1000));
            tokio::io::write_all(stream, to_send)
                .map_err(|e| warn!("Failed to send request: {}", e))
        })
        .and_then(|(stream, _)| {
            tokio::io::shutdown(stream)
                .map_err(|e| warn!("Failed to shut down stream: {}", e))
        })
        .map(move |_| info!("request complete"))
}

pub fn run_client() -> Result<()> {
    let remote =
        ::std::net::SocketAddr::from_str("127.0.0.1:4433").expect("Invalid url for client");

    let mut runtime = Runtime::new()?;

    let config = quicr::Config {
        protocols: vec![b"hq-11"[..].into()],
        keylog: None,
        ..quicr::Config::default()
    };

    let ticket = None;
    let mut builder = quicr::Endpoint::new();
    builder.config(config);
    let (endpoint, driver, _) = builder.bind("[::]:0")?;
    runtime.spawn(driver.map_err(|e| eprintln!("IO error: {}", e)));

    let message = Message::Ping { id: 999 };
    let serialized_message = rmp_serde::to_vec(&message).expect("Could not serialize message?!");

    let start = Instant::now();
    runtime.block_on(
        endpoint
            .connect(
                &remote,
                quicr::ClientConfig {
                    server_name: Some("localhost:4433"),
                    accept_insecure_certs: true,
                    session_ticket: ticket,
                    ..quicr::ClientConfig::default()
                },
            )?.map_err(|e| format_err!("failed to connect: {}", e))
            .and_then(move |conn| {
                eprintln!("connected at {}", duration_secs(&start.elapsed()));
                let conn = conn.connection;
                let stream = conn.open_bi();
                stream
                    .map_err(|e| format_err!("failed to open stream: {}", e))
                    .and_then(move |stream| {
                        eprintln!("stream opened at {}", duration_secs(&start.elapsed()));
                        tokio::io::write_all(stream, serialized_message)
                            .map_err(|e| format_err!("failed to send request: {}", e))
                    }).and_then(|(stream, _)| {
                        tokio::io::shutdown(stream)
                            .map_err(|e| format_err!("failed to shutdown stream: {}", e))
                    }).and_then(move |stream| {
                        let response_start = Instant::now();
                        eprintln!(
                            "request sent at {}",
                            duration_secs(&(response_start - start))
                        );
                        quicr::read_to_end(stream, usize::max_value())
                            .map_err(|e| format_err!("failed to read response: {}", e))
                            .map(move |x| (x, response_start))
                    }).and_then(move |((_, data), response_start)| {
                        let seconds = duration_secs(&response_start.elapsed());
                        eprintln!(
                            "response received in {}ms - {} KiB/s",
                            response_start.elapsed().subsec_millis(),
                            data.len() as f32 / (seconds * 1024.0)
                        );
                        let msg: ::std::result::Result<
                            Message,
                            rmp_serde::decode::Error,
                        > = rmp_serde::from_slice(&data);
                        debug!("Got response: {:?}", msg);
                        //io::stdout().write_all(&data).unwrap();
                        //io::stdout().flush().unwrap();
                        conn.close(0, b"done").map_err(|_| unreachable!())
                    }).map(|()| eprintln!("drained"))
            }),
    )?;

    Ok(())
}

pub fn run_server(options: ServerOpt) -> Result<()> {
    let mut runtime = Runtime::new()?;

    let mut builder = quicr::Endpoint::new();
    builder
        .config(quicr::Config {
            protocols: vec![b"hq-11"[..].into()],
            max_remote_bi_streams: 64,
            keylog: None,
            ..quicr::Config::default()
        }).listen();

    if let Some(key_path) = options.key {
        let mut key = Vec::new();
        File::open(&key_path)
            .context("failed to open private key")?
            .read_to_end(&mut key)
            .context("failed to read private key")?;
        builder
            .private_key_pem(&key)
            .context("failed to load private key")?;

        let cert_path = options.cert.unwrap(); // Ensured present by option parsing
        let mut cert = Vec::new();
        File::open(&cert_path)
            .context("failed to open certificate")?
            .read_to_end(&mut cert)
            .context("failed to read certificate")?;
        builder
            .certificate_pem(&cert)
            .context("failed to load certificate")?;
    } else {
        builder
            .generate_insecure_certificate()
            .context("failed to generate certificate")?;
    }

    let (_, driver, incoming) = builder.bind("[::]:4433")?;

    info!("Bound to port 4433, listening for incoming connections.");

    runtime.spawn(incoming.for_each(move |conn| {
        let quicr::NewConnection {
            incoming,
            connection,
        } = conn;
        info!(
            "got connection: {}, {}, {:?}",
            connection.remote_id(),
            connection.remote_address(),
            connection.protocol()
        );
        //let root = root.clone();
        current_thread::spawn(
            incoming
                .map_err(move |e| info!("connection terminated: {}", e))
                .for_each(move |stream| {
                    info!("Processing stream");
                    let stream = match stream {
                        quicr::NewStream::Bi(stream) => stream,
                        quicr::NewStream::Uni(_) => {
                            error!("client opened unidirectional stream");
                            return Ok(());
                        }
                    };
                    current_thread::spawn(run_ping(stream));
                    Ok(())
                }),
        );
        Ok(())
    }));

    runtime.block_on(driver)?;

    Ok(())
}

#[cfg(test)]
mod tests {

    use std::thread;

    use failure;
    use lazy_static;

    use peer;
    use ServerOpt;

    // This isn't necessarily the best way to do this but
    lazy_static! {
        static ref SERVER_THREAD: thread::JoinHandle<Result<(), failure::Error>> =
            thread::spawn(|| peer::run_server(ServerOpt {
                key: None,
                cert: None,
            }));
    }
    #[test]
    fn test_client_connection() {
        lazy_static::initialize(&SERVER_THREAD);
        let res = peer::run_client();
        assert!(res.is_ok())
    }
}
