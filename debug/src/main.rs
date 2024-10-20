use std::io;
use std::net::UdpSocket;
const SENDER_ADDR: &str = "0.0.0.0:9932";
const BROADCAST_ADDR: &str = "255.255.255.255:8080";

fn main() {
    {
        match UdpSocket::bind(SENDER_ADDR) {
            Ok(sock) => {
                sock.set_broadcast(true).expect("failed to set broadcast");
                loop {
                    let mut input = String::new();
                    io::stdin()
                        .read_line(&mut input)
                        .expect("failed to get input");
                    match sock.send_to(input.as_bytes(), BROADCAST_ADDR) {
                        Ok(v) => println!("send message : {}", &input[..v]),
                        Err(v) => println!("failed to send message:{}", v),
                    }
                }
                println!("stop sender");
            }
            Err(v) => println!("failed to start sender:{}", v),
        }
    }
    println!("aaaaaaaa");
}
