use crate::constants::{CLIENT_ID, PERMISSIONS};
use crate::discord::run_bot;

mod constants;

mod discord;

extern crate chrono;
extern crate chrono_tz;

#[macro_use]
extern crate derive_builder;

#[macro_use] extern crate log;
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

/*
it's hilarious how much faster it would have been to make this in e.g. django lol

critical path TODOs:

 * some sort of way to sync the messages w/ the races
   * perhaps the race table should have a channel/message id of the associated message?
   * perhaps

misc TODOs - not any special order:

 * Rate limit the bot
 * add a !commands command, or similar
 * pre-populate race reactions
 * i kinda think sending newly created races, possibly fully hydrated, off to some like mpsc-based
   handler might be the way of the hero?


 */