use crate::constants::{CLIENT_ID, PERMISSIONS};
use crate::discord::run_bot;

mod constants;

mod discord;


#[macro_use]
extern crate derive_builder;

#[macro_use] extern crate log;
extern crate env_logger;


#[tokio::main]
async fn main() {
    env_logger::init();
    let url = format!(
        "https://discord.com/oauth2/authorize?client_id={}&scope=bot&permissions={}",
        CLIENT_ID, PERMISSIONS
    );
    println!("{}", url);
    let jh = tokio::spawn(run_bot());
    jh.await.unwrap().unwrap();
}
