//! The identity sent with every request. A local listener records the request head, so the test
//! checks the header the client really sends.

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use mop_market::cache::Http;
    use mop_market::http::UreqHttp;

    /// Accepts one connection, answers it with an empty 200 and returns the request head it read.
    fn one_request() -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap() == 1 {
                head.push(byte[0]);
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .unwrap();
            String::from_utf8_lossy(&head).into_owned()
        });
        (url, server)
    }

    fn user_agent(head: &str) -> Option<&str> {
        head.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("user-agent")
                .then(|| value.trim())
        })
    }

    #[test]
    fn requests_carry_the_app_user_agent() {
        let (url, server) = one_request();
        UreqHttp::new().get(&url, None).unwrap();
        let head = server.join().unwrap();
        assert_eq!(
            user_agent(&head),
            Some("eve-means-of-profit/0.1.0"),
            "{head}"
        );
    }
}
