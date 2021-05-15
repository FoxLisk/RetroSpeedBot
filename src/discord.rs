use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;

use futures::stream::StreamExt;
use tokio::sync::RwLock;
use twilight_cache_inmemory::{InMemoryCache, ResourceType};
use twilight_command_parser::{Command, CommandParserConfig, Parser};
use twilight_gateway::{Cluster, Event};
use twilight_gateway::cluster::ShardScheme;
use twilight_http::{Client as HttpClient, Client};
use twilight_model::gateway::Intents;
use twilight_model::gateway::payload::{GuildCreate, MessageCreate};
use twilight_model::guild::{Permissions, Role};
use twilight_model::id::{ChannelId, GuildId, RoleId, UserId};

use super::constants::DISCORD_TOKEN;
use twilight_http::request::guild::role::CreateRole;
use crate::constants::FOXLISK_USER_ID;

struct BotState {
    http: Client,
    cluster: Cluster,
    cache: InMemoryCache,
    parser: Parser<'static>,
    // this should be split by guild
    roles: RwLock<HashMap<String, Role>>,
}

/*
https://discord.com/developers/docs/resources/guild#create-guild-role
    name	string	name of the role	"new role"
    permissions	string	bitwise value of the enabled/disabled permissions	@everyone permissions in guild
    color	integer	RGB color value	0
    hoist	boolean	whether the role should be displayed separately in the sidebar	false
    mentionable	boolean	whether the role should be mentionable	false
 */
#[derive(Builder, Clone, Debug)]
struct DesiredRole {
    name: String,
    #[builder(setter(strip_option), default)]
    permissions: Option<Permissions>,
    color: u32,
    #[builder(setter(strip_option), default)]
    hoist: Option<bool>,
    #[builder(setter(strip_option), default)]
    mentionable: Option<bool>,
}

impl<'a> DesiredRole {
    fn matches(&self, other: &Role) -> bool {
        if self.name != other.name {
            return false;
        }
        if let Some(perms) = self.permissions {
            if perms != other.permissions {
                return false;
            }
        }
        if self.color != other.color {
            return false;
        }
        if let Some(h) = self.hoist {
            if h != other.hoist {
                return false;
            }
        }
        if let Some(m) = self.mentionable {
            if m != other.mentionable {
                return false;
            }
        }
        true
    }

    fn create(&self, mut cr: CreateRole<'a>) -> CreateRole<'a> {
        cr = cr.name(self.name.clone()).color(self.color);
        if let Some(h) = self.hoist {
            cr = cr.hoist(h);
        }
        if let Some(p) = self.permissions {
            cr = cr.permissions(p);
        }
        if let Some(m) = self.mentionable {
            cr = cr.mentionable(m);
        }
        cr
    }
}

pub async fn run_bot() -> Result<(), Box<dyn Error + Send + Sync>> {
    // This is the default scheme. It will automatically create as many
    // shards as is suggested by Discord.
    let intents = Intents::GUILD_MESSAGES | Intents::GUILDS | Intents::GUILD_MESSAGE_REACTIONS;
    let cluster = Cluster::builder(DISCORD_TOKEN, intents)
        .shard_scheme(ShardScheme::Auto)
        .build()
        .await?;
    let cluster_spawn = cluster.clone();

    // Start all shards in the cluster in the background.
    tokio::spawn(async move {
        cluster_spawn.up().await;
    });

    // HTTP is separate from the gateway, so create a new client.
    let http = HttpClient::new(DISCORD_TOKEN);

    // Since we only care about new messages, make the cache only
    // cache new messages.

    let cache = InMemoryCache::builder()
        .resource_types(
            ResourceType::MESSAGE
                | ResourceType::VOICE_STATE
                | ResourceType::GUILD
                | ResourceType::CHANNEL
                | ResourceType::MEMBER
                | ResourceType::USER_CURRENT
                | ResourceType::ROLE,
        )
        .build();

    let mut config = CommandParserConfig::new();

    // (Use `Config::add_command` to add a single command)
    config.add_command("bot", true);
    config.add_prefix("!");

    let parser = Parser::new(config);

    // let shard_count = cluster.shards().len();

    let bot_state = Arc::new(BotState {
        http,
        cluster,
        cache,
        parser,
        roles: Default::default(),
    });

    let jh = tokio::spawn(handle_events(bot_state.clone()));
    jh.await.unwrap().unwrap();
    Ok(())
}

async fn handle_events(bot_state: Arc<BotState>) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut events = bot_state.cluster.events();
    while let Some((_, event)) = events.next().await {
        bot_state.cache.update(&event);
        tokio::spawn(handle_wrapper(event, bot_state.clone()));
    }

    Ok(())
}

async fn handle_wrapper(
    event: Event,
    bot_state: Arc<BotState>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    match handle_event(event, bot_state).await {
        Ok(()) => Ok(()),
        Err(e) => {
            println!("Unhandled error: {:?}", e.to_string());
            Err(e)
        }
    }
}

async fn handle_event(
    event: Event,
    bot_state: Arc<BotState>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    match event {
        Event::GuildCreate(msg) => {
            setup_roles(msg, bot_state.clone()).await;
        }
        Event::MessageCreate(msg) => match bot_state.parser.parse(msg.content.as_str()) {
            Some(Command { name: "bot", .. }) => {
                println!("{:?}", msg);
                bot_state
                    .http
                    .create_message(msg.channel_id)
                    .content("Help, I'm alive!")?
                    .await?;
            }
            _ => {}
        },
        Event::ShardConnected(_) => {
            println!("Discord: Shard connected!");
        }
        Event::VoiceStateUpdate(msg) => {
            println!("Discord: Voice State Update {:?}", msg);
        }
        Event::GatewayHeartbeatAck => {}
        _ => {}
    }

    Ok(())
}


async fn setup_roles(guild: Box<GuildCreate>, bot_state: Arc<BotState>) {
    for role in &guild.roles {
        println!("role: {:?}", role);
    }
    // let role_perms = Permissions::
    // let role_perms = PERM
    let desired_roles = vec![DesiredRoleBuilder::default()
        .name("active-racer".to_string())
        .color(0xE74C3C)
        .mentionable(true)
        .build()
        .unwrap()];

    let mut desired_roles_by_name: HashMap<String, DesiredRole> = Default::default();
    for role in desired_roles {
        desired_roles_by_name.insert(role.name.clone(), role);
    }

    for role in &guild.roles {
        if desired_roles_by_name.contains_key(&role.name) {
            let desired = desired_roles_by_name.get(&role.name).unwrap();
            if !desired.matches(role) {
                println!("Non-matching role found!! wtf {:?}", role);
            } else {
                desired_roles_by_name.remove(&role.name);
                {
                    let mut lock = bot_state.roles.write().await;
                    lock.insert(role.name.clone(), role.clone());
                }
            }
        }
    }

    for desired in desired_roles_by_name.values() {
    let c = bot_state.http.create_role(guild.id);
        println!("Creating role {:?}", desired);
        match desired.create(c).await{
            Ok(o) => {
                println!("Successful!");
            }
            Err(e) => {println!("Error creating role: {:?}", e);}
        }
    }

    for er in bot_state.roles.read().await.values() {
        bot_state.http.add_guild_member_role(guild.id, UserId::from(FOXLISK_USER_ID), er.id).await;
    }
}
