use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::fmt::{Display, Formatter};
use std::sync::Arc;

use futures::stream::StreamExt;
use tokio::sync::RwLock;
use twilight_cache_inmemory::{InMemoryCache, ResourceType};
use twilight_command_parser::{Command, CommandParserConfig, Parser};
use twilight_gateway::cluster::ShardScheme;
use twilight_gateway::{Cluster, Event};
use twilight_http::{Client as HttpClient, Client};
use twilight_model::gateway::payload::GuildCreate;
use twilight_model::gateway::Intents;
use twilight_model::guild::{Permissions, Role};
use twilight_model::id::{ChannelId, GuildId, RoleId, UserId};

use super::constants::DISCORD_TOKEN;
use crate::constants::FOXLISK_USER_ID;
use regex::Regex;
use tokio::time::Duration;
use twilight_http::request::guild::role::CreateRole;
use twilight_model::channel::Message;

const SCHEDULE_PATTERN: &str = r"There is a (?P<Game>.*) [rR]ace.*
(?P<Weekday>\w+), (?P<Month>) (?P<Day>\d+) at (?P<Hour>\d+):(?P<Minute>\d+) (?P<TZ>\w+)
(?P<Other>.*)";

struct BotState {
    http: Client,
    cluster: Cluster,
    cache: InMemoryCache,
    parser: Parser<'static>,

    // these should be split by guild
    roles: RwLock<HashMap<String, Role>>,
    channels: RwLock<HashMap<String, ChannelId>>,
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

    let http_client = HttpClient::new(DISCORD_TOKEN);

    let cache = InMemoryCache::builder()
        .resource_types(
            ResourceType::MESSAGE
                | ResourceType::GUILD
                | ResourceType::CHANNEL
                | ResourceType::MEMBER
                | ResourceType::USER_CURRENT
                | ResourceType::ROLE,
        )
        .build();

    let mut command_config = CommandParserConfig::new();

    // (Use `Config::add_command` to add a single command)
    command_config.add_command("bot", true);
    command_config.add_prefix("!");

    let parser = Parser::new(command_config);

    let bot_state = Arc::new(BotState {
        http: http_client,
        cluster,
        cache,
        parser,
        roles: Default::default(),
        channels: Default::default(),
    });

    let jh = tokio::spawn(handle_events(bot_state.clone()));

    jh.await.unwrap().unwrap();
    Ok(())
}

struct Schedule {}

#[derive(Debug)]
struct ScheduleError;

impl Display for ScheduleError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "bzzt")
    }
}

impl Error for ScheduleError {}

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
            warn!("Unhandled error: {:?}", e.to_string());
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
            setup_roles(&msg, bot_state.clone()).await;
            setup_channels(&msg, bot_state.clone()).await;
        }
        Event::ChannelUpdate(cu) => {
            // probably we could iterate thru bot_state.channels and change the key on the one
            // with the value matching this one. or maintain an id -> name map and use that
            // instead of iterating. otherwise *shrug*
            debug!("Channel update - should i care? {:?}", cu);
        }
        Event::MessageCreate(msg) => match bot_state.parser.parse(msg.content.as_str()) {
            Some(Command { name: "bot", .. }) => {
                bot_state
                    .http
                    .create_message(msg.channel_id)
                    .content("Help, I'm alive!")?
                    .await?;
            }
            _ => {}
        },
        Event::ShardConnected(_) => {
            debug!("Discord: Shard connected!");
        }
        Event::GatewayHeartbeatAck => {}
        _ => {}
    }

    Ok(())
}

async fn setup_channels(guild: &Box<GuildCreate>, bot_state: Arc<BotState>) {
    let mut lock = bot_state.channels.write().await;
    for c in &guild.channels {
        debug!("Inserting channel `{}`", c.name());
        lock.insert(c.name().to_string(), c.id());
    }
}

async fn setup_roles(guild: &Box<GuildCreate>, bot_state: Arc<BotState>) {
    // for role in &guild.roles {
    // println!("role: {:?}", role);
    // }
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
                warn!("Non-matching role found!! wtf {:?}", role);
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
        info!("Creating role {:?}", desired);
        match desired.create(c).await {
            Ok(_) => {
                debug!("Successfuly created role!");
            }
            Err(e) => {
                warn!("Error creating role: {:?}", e);
            }
        }
    }

    for er in bot_state.roles.read().await.values() {
        match bot_state
            .http
            .add_guild_member_role(guild.id, UserId::from(FOXLISK_USER_ID), er.id)
            .await
        {
            Ok(_) => {}
            Err(e) => {
                warn!("Failed to assign to role: {:?}", e);
            }
        }
    }
}
