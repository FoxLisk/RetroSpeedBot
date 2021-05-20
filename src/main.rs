use crate::constants::{CLIENT_ID};
use crate::discord::run_bot;
use twilight_model::guild::Permissions;

mod constants;
mod discord;
mod models;

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

    let required_permissions = Permissions::MANAGE_CHANNELS
        | Permissions::ADD_REACTIONS
        | Permissions::VIEW_CHANNEL
        | Permissions::SEND_MESSAGES
        | Permissions::MANAGE_MESSAGES
        | Permissions::READ_MESSAGE_HISTORY
        | Permissions::MANAGE_ROLES;
    println!("expected perms: {:?}", required_permissions.bits());

    // TODO: probably need some user management powers here
    let url = format!(
        "https://discord.com/oauth2/authorize?client_id={}&scope=bot&permissions={}",
        CLIENT_ID, required_permissions.bits()
    );
    println!("{}", url);
    let jh = tokio::spawn(run_bot());
    jh.await.unwrap().unwrap();
}
