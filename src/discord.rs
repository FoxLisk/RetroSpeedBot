use std::collections::{HashMap, HashSet};
use std::env::var;
use std::error::Error;
use std::sync::Arc;

use futures::stream::StreamExt;
use tokio::sync::RwLock;
use twilight_cache_inmemory::{InMemoryCache, ResourceType};
use twilight_command_parser::{Arguments, Command, CommandParserConfig, Parser};
use twilight_gateway::cluster::ShardScheme;
use twilight_gateway::{Cluster, Event};
use twilight_http::{Client as HttpClient, Client};
use twilight_model::gateway::payload::{GuildCreate, MessageCreate};
use twilight_model::gateway::Intents;
use twilight_model::guild::{Permissions, Role};
use twilight_model::id::{ChannelId, EmojiId, RoleId, UserId};

use super::constants::DISCORD_TOKEN;
use crate::constants::{FOXLISK_USER_ID, SCHEDULING_CHANNEL_NAME};
use twilight_http::request::guild::role::CreateRole;

use chrono::{DateTime, LocalResult, NaiveDateTime, TimeZone};
use chrono_tz::Tz;
use chrono_tz::US::Eastern;
use futures::TryStreamExt;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::SqlitePool;
use std::iter::FromIterator;
use twilight_model::channel::ChannelType;

struct BotState {
    http: Client,
    cluster: Cluster,
    cache: InMemoryCache,
    parser: Parser<'static>,
    // these should be split by guild
    roles: RwLock<HashMap<String, Role>>,
    channels: RwLock<HashMap<String, ChannelId>>,
    emojis: RwLock<HashMap<String, EmojiId>>,
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

    // TODO: manage games and categories via command

    // TODO: use a higher-powered command parser
    command_config.add_command("bot", true);
    command_config.add_command("listgames", true);
    command_config.add_command("listcategories", true);
    command_config.add_command("newrace", true);
    command_config.add_prefix("!");

    let parser = Parser::new(command_config);

    let bot_state = Arc::new(BotState {
        http: http_client,
        cluster,
        cache,
        parser,
        roles: Default::default(),
        channels: Default::default(),
        emojis: Default::default(),
    });

    // let (queries_send, queries_recv) = tokio::sync::mpsc::channel(1000);

    let jh = tokio::spawn(handle_events(bot_state.clone()));
    // let sql_jh = tokio::spawn(sqlite(sqlite_db_path, queries_recv));

    jh.await.unwrap().unwrap();
    Ok(())
}

async fn handle_events(bot_state: Arc<BotState>) -> Result<(), Box<dyn Error + Send + Sync>> {
    let sqlite_db_path = var("SQLITE_DB_PATH").unwrap();
    // use a SqliteConnectOptions instead of a hardcoded queryparam?
    let path_with_params = format!("{}?mode=rwc", sqlite_db_path);
    let pool = SqlitePoolOptions::new()
        .max_connections(12)
        .connect(&path_with_params)
        .await
        .unwrap();
    let games = get_games(&pool).await;
    for g in games {
        println!("Game: {:?}", g);
    }
    {
        let mut events = bot_state.cluster.events();
        while let Some((_, event)) = events.next().await {
            bot_state.cache.update(&event);
            handle_wrapper(event, bot_state.clone(), &pool).await;
        }
    }

    Ok(())
}

async fn handle_wrapper(event: Event, bot_state: Arc<BotState>, pool: &SqlitePool) {
    match handle_event(event, bot_state, pool).await {
        Ok(()) => {}
        Err(e) => {
            warn!("Unhandled error: {:?}", e.to_string());
        }
    }
}

async fn handle_event(
    event: Event,
    bot_state: Arc<BotState>,
    pool: &SqlitePool,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    match event {
        Event::GuildCreate(msg) => {
            setup_roles(&msg, bot_state.clone()).await;
            setup_channels(&msg, bot_state.clone()).await;
            setup_emojis(&msg, bot_state.clone()).await;
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
            Some(Command {
                name: "listgames", ..
            }) => list_games(&msg, bot_state.clone(), pool).await,
            Some(Command {
                arguments,
                name: "listcategories",
                ..
            }) => list_categories(&msg, arguments, bot_state.clone(), pool).await,
            Some(Command {
                arguments,
                name: "newrace",
                ..
            }) => add_race(&msg, arguments, bot_state.clone(), pool).await,
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

async fn list_categories(
    msg: &Box<MessageCreate>,
    args: Arguments<'_>,
    bot_state: Arc<BotState>,
    pool: &SqlitePool,
) {
    let contents = _list_categories(args, pool).await;
    bot_state
        .http
        .create_message(msg.channel_id)
        .content(contents)
        .unwrap()
        .await;
}

async fn _list_categories(mut args: Arguments<'_>, pool: &SqlitePool) -> String {
    let game_name = match args.next() {
        Some(game) => game,
        None => {
            return "Please specify game: !listcategories <game>".to_owned();
        }
    };
    let game = match get_game(game_name, pool).await {
        Some(g) => g,
        None => {
            return "No game found with that name".to_owned();
        }
    };

    let categories = get_categories(&game, pool).await;

    let mut msg_parts = vec![format!("Available categories for {}:", game.name_pretty)];
    msg_parts.extend(
        categories
            .iter()
            .map(|c| format!("* {} ({})", c.name_pretty, c.name)),
    );

    msg_parts.join("\n")
}

async fn add_race(
    msg: &Box<MessageCreate>,
    args: Arguments<'_>,
    bot_state: Arc<BotState>,
    pool: &SqlitePool,
) {
    // note: this is mega annoying and probably pretty slow to do in here.
    let user_roles: HashSet<RoleId> =
        HashSet::from_iter(msg.member.as_ref().unwrap().roles.iter().cloned());
    let guild_roles = bot_state.cache.guild_roles(msg.guild_id.unwrap()).unwrap();
    let mut valid_role_names = HashSet::new();
    valid_role_names.insert("Moderator".to_owned());
    valid_role_names.insert("Admin".to_owned());

    let mut permitted = false;
    for r in &guild_roles {
        let role = bot_state.cache.role(r.clone()).unwrap();
        if valid_role_names.contains(&role.name) && user_roles.contains(&role.id) {
            permitted = true;
            break;
        }
    }

    if !permitted {
        bot_state
            .http
            .create_message(msg.channel_id)
            .content("You are not authorized to create races.")
            .unwrap()
            .await;
        return;
    }

    let (reply, schedule) = _add_race(args, bot_state.clone(), pool).await;
    bot_state
        .http
        .create_message(msg.channel_id)
        .content(reply)
        .unwrap()
        .await;

    if let Some(schedule_message) = schedule {
        let schedule_channel_id = {
            let lock = bot_state.channels.read().await;
            match lock.get(SCHEDULING_CHANNEL_NAME) {
                None => {
                    warn!("No scheduling channel found");
                    None
                }
                Some(cid) => Some(cid.clone()),
            }
        };
        if let Some(cid) = schedule_channel_id {
            bot_state
                .http
                .create_message(cid)
                .content(schedule_message)
                .unwrap()
                .await;
        }
    }
    // let scheuld_message = format!("There is a race for {} at {}. "
}

async fn _add_race(
    mut args: Arguments<'_>,
    bot_state: Arc<BotState>,
    pool: &SqlitePool,
) -> (String, Option<String>) {
    let syntax_error = "Please use the following format: !newrace <game alias> <category alias> <time>. For example: `!newrace alttp ms 6/9/2021 11:00pm. *Convert to Eastern time first*";
    let game_name = match args.next() {
        Some(game) => game,
        None => {
            return (syntax_error.to_owned(), None);
        }
    };

    let cat_name = match args.next() {
        Some(cat) => cat,
        None => {
            return (syntax_error.to_owned(), None);
        }
    };

    let time = match args.into_remainder() {
        Some(t) => t,
        None => {
            return (syntax_error.to_owned(), None);
        }
    };

    let occurs = match parse_time(time) {
        Some(dt) => dt,
        None => {
            return (syntax_error.to_owned(), None);
        }
    };

    let game = match get_game(game_name, pool).await {
        Some(g) => g,
        None => {
            return (
                "No game found with that name. Try !listgames".to_owned(),
                None,
            );
        }
    };

    let cat = match get_category(&game, cat_name, pool).await {
        Some(c) => c,
        None => {
            return (
                format!(
                    "No matching category found. try !listcategories {}",
                    game.name
                ),
                None,
            );
        }
    };

    match create_race(&game, &cat, occurs, pool).await {
        Some(r) => r,
        None => {
            return (
                "Unknown error creating the race. Bug Fox about it.".to_owned(),
                None,
            );
        }
    };

    let racer_react = {
        let lock = bot_state.emojis.read().await;
        match lock.get("raisinghand") {
            None => {
                warn!("Can't find raising hand emoji");
                ":thumbup:".to_owned()
            }
            Some(eid) => {
                format!("<:raisinghand:{}>", eid)
            }
        }
    };

    let schedule_content = format!(
        "There will be a race of {} - {} on {}.\
If you are racing, react with {}
If you are available to commentate, react with :microphone2:
If you are able to restream, react with :tv:",
        game.name_pretty,
        cat.name_pretty,
        occurs.format("%A, %B %d at %I:%M%p %Z"),
        racer_react
    );

    ("Race created!".to_string(), Some(schedule_content))
}

async fn create_race(
    game: &Game,
    category: &Category,
    occurs: DateTime<Tz>,
    pool: &SqlitePool,
) -> Option<Race> {
    let ts = occurs.timestamp();
    let q = sqlx::query!("INSERT INTO race (game_id, category_id, occurs) VALUES (?, ?, ?); SELECT last_insert_rowid() as rowid;", game.id, category.id, ts);
    match q.fetch_one(pool).await {
        Ok(e) => {
            debug!("create_race got me a {:?}", e);
            Some(Race {
                id: e.rowid as i64,
                game_id: game.id,
                category_id: category.id,
                occurs,
            })
        }
        Err(e) => {
            error!("error creating race: {:?}", e);
            None
        }
    }
}

fn parse_time(time_str: &str) -> Option<DateTime<Tz>> {
    let normalized = time_str.to_ascii_lowercase();
    println!("Parsing date from {}", normalized);
    match NaiveDateTime::parse_from_str(&normalized, "%m/%d/%Y %I:%M%P") {
        Ok(dt) => match Eastern.from_local_datetime(&dt) {
            LocalResult::Single(single) => Some(single),
            LocalResult::Ambiguous(a, b) => {
                println!("Ambiguous result... what are the values??? {:?} {:?}", a, b);
                warn!("Ambiguous result... what are the values??? {:?} {:?}", a, b);
                Some(a)
            }
            LocalResult::None => {
                println!("Failed to parse");
                warn!("Failed to parse");
                None
            }
        },
        Err(e) => {
            println!("Error parsing date: {}", e);
            info!("Error parsing date: {}", e);
            None
        }
    }
}

async fn list_games(msg: &Box<MessageCreate>, bot_state: Arc<BotState>, pool: &SqlitePool) {
    let games = get_games(pool).await;
    let mut msg_parts = vec!["Available games:".to_owned()];
    msg_parts.extend(
        games
            .iter()
            .map(|g| format!("* {} ({})", g.name_pretty, g.name)),
    );

    let contents = msg_parts.join("\n");
    // TODO: actually check content length if we get enough games
    bot_state
        .http
        .create_message(msg.channel_id)
        .content(contents)
        .unwrap()
        .await;
}

// Would love to have a real ORM... oh well
// it's probably actually better to have id: Option<i64> so we can create models that aren't
// DB-backed
#[derive(Debug)]
struct Game {
    id: i64,
    name: String,
    name_pretty: String,
}

// TODO: hmmm... how to handle FKs? i think *for now* it's fine to just do stuff top down.
//       probably eventually we want some kind of hydration
struct Category {
    id: i64,
    game_id: i64,
    name: String,
    name_pretty: String,
}

struct Race {
    id: i64,
    game_id: i64,
    category_id: i64,
    occurs: DateTime<Tz>,
}

async fn get_game(name: &str, pool: &SqlitePool) -> Option<Game> {
    let q = sqlx::query_as!(
        Game,
        "SELECT id, name, name_pretty FROM game WHERE name = ?",
        name
    );
    match q.fetch_one(pool).await {
        Ok(r) => Some(r),
        Err(e) => {
            warn!("Error fetching game: {:?}", e);
            None
        }
    }
}

async fn get_category(game: &Game, name: &str, pool: &SqlitePool) -> Option<Category> {
    debug!(
        "Getting category {} for game (name {} id {}) ",
        name, game.name, game.id
    );
    let q = sqlx::query_as!(
        Category,
        "SELECT id, game_id, name, name_pretty FROM category WHERE name = ? AND game_id = ?",
        name,
        game.id,
    );

    match q.fetch_one(pool).await {
        Ok(r) => Some(r),
        Err(e) => {
            warn!("Error fetching category: {:?}", e);
            None
        }
    }
}

async fn get_games(pool: &SqlitePool) -> Vec<Game> {
    let q = sqlx::query_as!(Game, "SELECT id, name, name_pretty FROM game");
    let mut rows = q.fetch(pool);
    let mut games = vec![];
    while let r = rows.try_next().await {
        match r {
            Ok(mg) => match mg {
                Some(g) => {
                    games.push(g);
                }
                None => {
                    break;
                }
            },
            Err(e) => {
                warn!("Error fetching row: {:?}", e);
            }
        }
    }

    games
}

async fn get_categories(game: &Game, pool: &SqlitePool) -> Vec<Category> {
    debug!(
        "Getting categories for game (name {} id {})",
        game.name, game.id
    );
    let q = sqlx::query_as!(
        Category,
        "SELECT id, game_id, name, name_pretty  FROM category WHERE game_id = ?",
        game.id
    );
    let mut rows = q.fetch(pool);
    let mut categories = vec![];
    while let r = rows.try_next().await {
        match r {
            Ok(mc) => match mc {
                Some(c) => {
                    categories.push(c);
                }
                None => {
                    break;
                }
            },
            Err(e) => {
                warn!("Error fetching row: {:?}", e);
            }
        }
    }

    categories
}

async fn setup_emojis(guild: &Box<GuildCreate>, bot_state: Arc<BotState>) {
    let mut lock = bot_state.emojis.write().await;
    for e in &guild.emojis {
        lock.insert(e.name.to_string(), e.id);
    }
}

async fn setup_channels(guild: &Box<GuildCreate>, bot_state: Arc<BotState>) {
    let mut has_schedule_channel = false;
    let mut lock = bot_state.channels.write().await;
    for c in &guild.channels {
        debug!("Inserting channel `{}` {}", c.name(), c.id());
        lock.insert(c.name().to_string(), c.id());
        if c.name() == SCHEDULING_CHANNEL_NAME {
            has_schedule_channel = true;
        }
    }
    if !has_schedule_channel {
        match bot_state
            .http
            .create_guild_channel(guild.id.clone(), SCHEDULING_CHANNEL_NAME)
        {
            Ok(chan) => match chan
                .kind(ChannelType::GuildText)
                .parent_id(798390141496197212)
                .await
            {
                Ok(created) => {
                    lock.insert(created.name().to_string(), created.id());
                }
                Err(e) => {
                    warn!("Error creating scheduling channel: {}", e);
                }
            },
            Err(e) => {
                warn!("Error creating scheduling channel: {}", e);
            }
        }
    }
}

async fn setup_roles(guild: &Box<GuildCreate>, bot_state: Arc<BotState>) {
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

#[cfg(test)]
mod test {
    use crate::discord::parse_time;
    use chrono::{Datelike, NaiveDateTime, Timelike};

    fn init() {
        let _ = env_logger::builder().is_test(true).try_init();
    }

    #[test]
    fn test_datetime_stuff() {
        let ndt = NaiveDateTime::parse_from_str("06/09/2021 11:00pm", "%m/%d/%Y %I:%M%P");
        ndt.unwrap();
    }

    #[test]
    fn test_parse_time() {
        let odt = parse_time("06/09/2021 11:00pm");
        assert!(odt.is_some());
        let dt = odt.unwrap();
        assert_eq!(6, dt.month());
        assert_eq!(9, dt.day());
        assert_eq!(2021, dt.year());
        assert_eq!(23, dt.hour());
        assert_eq!(0, dt.minute());
        assert_eq!(0, dt.second());
        assert_eq!("2021-06-09T23:00:00-04:00", dt.to_rfc3339());
        assert_eq!(1623294000, dt.timestamp());
    }
}