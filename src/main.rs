use crate::constants::{CLIENT_ID, PERMISSIONS};
use crate::discord::run_bot;

mod constants;

mod discord;

extern crate chrono;
extern crate chrono_tz;

#[macro_use]
extern crate derive_builder;

#[macro_use]
extern crate log;
extern crate env_logger;

#[tokio::main]
async fn main() {
    env_logger::init();

    // TODO: probably need some user management powers here
    let url = format!(
        "https://discord.com/oauth2/authorize?client_id={}&scope=bot&permissions={}",
        CLIENT_ID, PERMISSIONS
    );
    println!("{}", url);
    let jh = tokio::spawn(run_bot());
    jh.await.unwrap().unwrap();
}
