use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::sync::Arc;

use custom_error::custom_error;

use futures::stream::StreamExt;
use tokio::sync::RwLock;
use twilight_cache_inmemory::{InMemoryCache, ResourceType};
use twilight_command_parser::{Arguments, Command, CommandParserConfig, Parser};
use twilight_gateway::cluster::ShardScheme;
use twilight_gateway::{Cluster, Event};
use twilight_http::request::channel::reaction::RequestReactionType;
use twilight_http::{Client as HttpClient, Client};
use twilight_model::gateway::payload::{GuildCreate, MessageCreate};
use twilight_model::gateway::Intents;
use twilight_model::guild::{Emoji, Permissions, Role};
use twilight_model::id::{ChannelId, EmojiId, GuildId, MessageId, RoleId, UserId};

use crate::constants::{
    FOXLISK_USER_ID, NOTIFY_BEFORE_RACE_SECS, RACING_EMOJI_NAME, SCHEDULING_CHANNEL_NAME,
};
use twilight_http::request::guild::role::CreateRole;

use chrono::{DateTime, Duration as CDuration, Local, LocalResult, NaiveDateTime, TimeZone};
use chrono_tz::Tz;
use chrono_tz::US::Eastern;
use futures::TryStreamExt;
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::SqlitePool;
use std::fmt::{Display, Formatter};
use std::iter::FromIterator;
use std::str::FromStr;
use tokio::time::Duration;
use twilight_model::channel::message::MessageReaction;
use twilight_model::channel::{ChannelType, ReactionType};
use twilight_model::user::User;

struct BotState {
    http: Client,
    cluster: Cluster,
    cache: InMemoryCache,
    parser: Parser<'static>,
    // these should be split by guild
    roles: RwLock<HashMap<String, Role>>,
    channels: RwLock<HashMap<String, ChannelId>>,
    emojis: RwLock<HashMap<String, Emoji>>,
    // TODO: this can't possibly be the best way to do this lol
    guild_id: RwLock<Option<GuildId>>,
}

impl BotState {
    async fn get_guild_id(&self) -> Option<GuildId> {
        let lock = self.guild_id.read().await;
        (*lock).clone()
    }

    async fn get_role(&self, name: &str) -> Option<Role> {
        let lock = self.roles.read().await;
        match lock.get(name) {
            Some(r) => Some(r.clone()),
            None => None,
        }
    }
}

macro_rules! loop_until_success {
    ($e:expr) => {
        loop {
            match $e {
                Some(c) => {
                    break c;
                }
                None => {}
            };

            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
        }
    };
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

    let discord_token = dotenv::var("DISCORD_TOKEN").unwrap();
    let cluster = Cluster::builder(discord_token.clone(), intents)
        .shard_scheme(ShardScheme::Auto)
        .build()
        .await?;
    let cluster_spawn = cluster.clone();

    // Start all shards in the cluster in the background.
    tokio::spawn(async move {
        cluster_spawn.up().await;
    });

    let http_client = HttpClient::new(discord_token);

    let cache = InMemoryCache::builder()
        .resource_types(
            ResourceType::MESSAGE
                | ResourceType::GUILD
                | ResourceType::CHANNEL
                | ResourceType::MEMBER
                | ResourceType::USER_CURRENT
                | ResourceType::ROLE
                | ResourceType::REACTION,
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
        guild_id: Default::default(),
    });

    let pool = get_pool().await.unwrap();

    let jh = tokio::spawn(handle_events(bot_state.clone(), pool.clone()));
    let cjh = tokio::spawn(cron(bot_state.clone(), pool.clone()));

    jh.await.unwrap().unwrap();
    cjh.await.unwrap();
    Ok(())
}

async fn get_pool() -> Result<SqlitePool, sqlx::Error> {
    let sqlite_db_path = dotenv::var("DATABASE_URL").unwrap();
    // use a SqliteConnectOptions instead of a hardcoded queryparam?
    let path_with_params = format!("{}?mode=rwc", sqlite_db_path);
    SqlitePoolOptions::new()
        .max_connections(12)
        .connect(&path_with_params)
        .await
}

async fn cron(bot_state: Arc<BotState>, pool: SqlitePool) {
    /*
    do we need something like a users table and <users, racers>/<users, commentators>/<users, restreamers> join tables?
    I'd rather avoid that - it's a lot of extra work, at least without ORM autogeneration magic. I think we can handle it
    all reactively... :\

    Basic design of this subsystem:
    * Every $DURATION we wake up and check state against db.
    * if there is a race in the next $SOON, do some stuff about it:
      1. if this is the first time this race has been $SOON:
        1. set race to ACTIVE
        1. set roles
            * i think we want, like, "active-X-interested" and "active-X-confirmed" roles for racer/restreamer/commentator
            * somewhere else we need to be responding to reacts by adding the roles *if* the race is $SOON
            * that almost definitely implies we want an in-memory cache of the race, unfortunately. that's kind of annoying.
        1. send messages
            * send a message that has a @active-X-interested roles in it telling people to react if they're ready
            * we can react to this elsewhere or perhaps, if it's easier, we can simply check each time we wake up
      1. otherwise:
        1. if we've waited $LONG_ENOUGH, bug people again
      1. unsetting this stuff is probably going to require an !endrace from a mod for now. might hook into racetime in the future
     */
    let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60 * 2));
    let scheduling_channel: ChannelId =
        loop_until_success!(get_scheduling_channel(bot_state.clone()).await);

    let racing_react: ReactionType = loop_until_success!({
        let lock = bot_state.emojis.read().await;
        lock.get(RACING_EMOJI_NAME).map(|e| ReactionType::Custom {
            animated: false,
            id: e.id,
            name: Some(e.name.clone()),
        })
    });

    let unconfirmed_racer_role: Role =
        loop_until_success!({ bot_state.get_role("unconfirmed-racer").await });

    let commentating_react = ReactionType::Unicode {
        name: "microphone2".to_string(),
    };
    let restreaming_react = ReactionType::Unicode {
        name: "tv".to_string(),
    };

    debug!("Cron has found necessary state");
    loop {
        interval.tick().await;
        debug!("Starting cron tick");

        let races = get_upcoming_races(Duration::from_secs(NOTIFY_BEFORE_RACE_SECS), &pool).await;
        for mut race in races {
            let msg = match bot_state
                .cache
                .message(scheduling_channel, race.get_message_id().unwrap())
            {
                Some(m) => m,
                None => {
                    warn!("No scheduling message found for race {}", race.id);
                    continue;
                }
            };

            // NB: as noted when building the cache, the msg.reactions field is not actually useful here
            let racing_reactions = match get_reactions(
                scheduling_channel,
                race.get_message_id().unwrap(),
                racing_react.clone(),
                bot_state.clone(),
            )
            .await
            {
                Ok(users) => users,
                Err(e) => {
                    warn!("Error fetching racing reactions for race {}", race.id);
                    continue;
                }
            };

            for user in racing_reactions {
                add_role(user, &unconfirmed_racer_role, bot_state.clone()).await;
            }

            let fh = {
                let lock = bot_state.channels.read().await;
                lock.get("🦊fox-hole").unwrap().clone()
            };
            bot_state
                .http
                .create_message(fh)
                .content(format!(
                    "<@&{}> hi fox good job testing you're so smart",
                    unconfirmed_racer_role.id
                ))
                .unwrap()
                .await;

            race.set_state(RaceState::ACTIVE);
            update_race(&race, &pool).await;
        }

        /*
        for race in races:
            check reacts on the scheduling message
            set interested-* roles
            @message ppl (where? do we want a dedicated channel for this?)
            set race to ACTIVE

        let active = get_active_races()
        for race in active:
            check reacts on the @message from before
            swap ready-* roles out for active-* roles
            if (some condition such as (mins until race % 5 == 0)) {
                @message interested-* ppl who haven't confirmed yet
            }
            if the start time has passed, shut up
                or perhaps keep pinging until a mod says !startrace or something?
            if start time passed, like, 2 hours ago or something, remove the racer roles
                one problem here is tracking racers, potentially, if we've restarted since setting stuff in memory

         */
    }
}

custom_error! { AddRoleError{err: String} = "Error adding role to user: {err}" }

// this should take GuildId but since this is single-guild for now we can sneak it off of the role
async fn add_role(user: User, role: &Role, bot_state: Arc<BotState>) -> Result<(), AddRoleError> {
    let gid = match bot_state.get_guild_id().await {
        Some(g) => g,
        None => {
            return Err(AddRoleError {
                err: "Cant find guild id???".to_string(),
            });
        }
    };
    match bot_state
        .http
        .add_guild_member_role(gid, user.id, role.id.clone())
        .await
    {
        Ok(()) => Ok(()),
        Err(e) => {
            warn!("Error adding role");
            Err(AddRoleError {
                err: "Error adding role".to_string(),
            })
        }
    }
}

/// N.B. this will return up to 100 reactions. If that ever becomes an issue, well, good problem to have!
async fn get_reactions(
    cid: ChannelId,
    mid: MessageId,
    react: ReactionType,
    bot_state: Arc<BotState>,
) -> Result<Vec<User>, twilight_http::error::Error> {
    bot_state
        .http
        .reactions(cid, mid, RequestReactionType::from(react))
        .await
}

async fn get_scheduling_channel(bot_state: Arc<BotState>) -> Option<ChannelId> {
    let lock = bot_state.channels.read().await;
    lock.get(SCHEDULING_CHANNEL_NAME).map(|f| f.clone())
}

async fn handle_events(
    bot_state: Arc<BotState>,
    pool: SqlitePool,
) -> Result<(), Box<dyn Error + Send + Sync>> {
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
            if msg.name != "RetroSpeedRuns" {
                warn!("Unexpected guild found! {}", msg.name);
                return Ok(());
            }
            {
                let mut lock = bot_state.guild_id.write().await;
                *lock = Some(msg.id.clone());
            }
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

        // Check if these are on races that are *active* - if so, add/remove roles
        Event::ReactionAdd(ra) => {
            debug!("Reaction added: {:?}", ra);
            /*
            if ra.emoji is one of the ones we care about:
                let msg_id = get_msg(ra.message_id);
                // this is a bit confusing because we're going to get the same *race* object in either case,
                // the difference is just if we got it via scheduling msg or via ready-up msg.
                if let Some(upcoming_race) = get_race_by_scheduling_msg_id(msg_id):
                    // this is a scheduling message, players are interested
                    add interested-* role to player if they don't have it or active-* role
                else if let Some(active_race) = get_race_by_active_msg_id(msg_id):
                    // this is an active message, players are readying up
                    remove interested-* role from player
                    add active-* role if they don't have it

                the ReactionRemove code should be very similar to this, with the addition that if
                someone removes their ready emoji it's unclear if they should be moved back to interested-
                or if they should be removed entirely.
             */
        }
        Event::ReactionRemove(rr) => {
            debug!("Reaction removed: {:?}", rr);
        }
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

    let (reply, schedule, race) = _add_race(args, bot_state.clone(), pool).await;
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
            match bot_state
                .http
                .create_message(cid)
                .content(schedule_message)
                .unwrap()
                .await
            {
                Ok(ok) => {
                    if let Some(mut r) = race {
                        r.set_message_id(ok.id);
                        update_race(&r, pool).await;
                    }
                }
                Err(_) => {}
            }
        }
    }
}

// TODO this is kind of a shitty return value at this point lol
async fn _add_race(
    mut args: Arguments<'_>,
    bot_state: Arc<BotState>,
    pool: &SqlitePool,
) -> (String, Option<String>, Option<Race>) {
    let syntax_error = "Please use the following format: !newrace <game alias> <category alias> <time>. For example: `!newrace alttp ms 6/9/2021 11:00pm. *Convert to Eastern time first*";
    let game_name = match args.next() {
        Some(game) => game,
        None => {
            return (syntax_error.to_owned(), None, None);
        }
    };

    let cat_name = match args.next() {
        Some(cat) => cat,
        None => {
            return (syntax_error.to_owned(), None, None);
        }
    };

    let time = match args.into_remainder() {
        Some(t) => t,
        None => {
            return (syntax_error.to_owned(), None, None);
        }
    };

    let occurs = match parse_time(time) {
        Some(dt) => dt,
        None => {
            return (syntax_error.to_owned(), None, None);
        }
    };
    // TODO: don't create races in the past

    let game = match get_game(game_name, pool).await {
        Some(g) => g,
        None => {
            return (
                "No game found with that name. Try !listgames".to_owned(),
                None,
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
                None,
            );
        }
    };

    let r = match create_race(&game, &cat, occurs, pool).await {
        Some(r) => Some(r),
        None => {
            return (
                "Unknown error creating the race. Bug Fox about it.".to_owned(),
                None,
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
            Some(e) => {
                format!("<:raisinghand:{}>", e.id)
            }
        }
    };

    let schedule_content = format!(
        "There will be a race of {} - {} on {}.

If you are racing, react with {}
If you are available to commentate, react with :microphone2:
If you are able to restream, react with :tv:
",
        game.name_pretty,
        cat.name_pretty,
        occurs.format("%A, %B %d at %I:%M%p %Z"),
        racer_react
    );

    ("Race created!".to_string(), Some(schedule_content), r)
}

// TODO: This creates a race with null message_id and state SCHEDULED, always. Is that bad?
async fn create_race(
    game: &Game,
    category: &Category,
    occurs: DateTime<Tz>,
    pool: &SqlitePool,
) -> Option<Race> {
    let ts = occurs.timestamp();
    let state = RaceState::SCHEDULED.to_string();
    let q = sqlx::query!("INSERT INTO race (game_id, category_id, occurs, state) VALUES (?, ?, ?, ?); SELECT last_insert_rowid() as rowid;", game.id, category.id, ts, state);
    match q.fetch_one(pool).await {
        Ok(e) => {
            debug!("create_race got me a {:?}", e);
            Some(Race {
                id: e.rowid as i64,
                game_id: game.id,
                category_id: category.id,
                occurs: ts,
                message_id: None,
                state,
            })
        }
        Err(e) => {
            error!("error creating race: {:?}", e);
            None
        }
    }
}

async fn update_race(race: &Race, pool: &SqlitePool) -> sqlx::Result<()> {
    let q = sqlx::query!(
        "UPDATE race SET game_id = ?, category_id = ?, occurs = ?, message_id = ?, state = ? WHERE id = ?",
        race.game_id,
        race.category_id,
        race.occurs,
        race.message_id,
        race.state,
        race.id,
    );
    debug!("Updating race: {:?}", race);
    match q.execute(pool).await {
        Ok(_) => Ok(()),
        Err(e) => Err(e),
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
#[derive(Debug, PartialEq)]
struct Game {
    id: i64,
    name: String,
    name_pretty: String,
}

// TODO: hmmm... how to handle FKs? i think *for now* it's fine to just do stuff top down.
//       probably eventually we want some kind of hydration

#[derive(Debug, PartialEq)]
struct Category {
    id: i64,
    game_id: i64,
    name: String,
    name_pretty: String,
}

#[derive(Debug, PartialEq)]
enum RaceState {
    SCHEDULED,
    ACTIVE,
    COMPLETED,
}

impl Display for RaceState {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                RaceState::SCHEDULED => "SCHEDULED",
                RaceState::ACTIVE => "ACTIVE",
                RaceState::COMPLETED => "COMPLETED",
            }
        )
    }
}

#[derive(Debug)]
struct ParseError;
impl Display for ParseError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Parse error")
    }
}
impl Error for ParseError {}

impl FromStr for RaceState {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "SCHEDULED" => Ok(RaceState::SCHEDULED),
            "ACTIVE" => Ok(RaceState::ACTIVE),
            "COMPLETED" => Ok(RaceState::COMPLETED),
            _ => Err(ParseError),
        }
    }
}

#[derive(Debug, PartialEq)]
struct Race {
    id: i64,
    // N.B. game_id is not strictly necessary in this struct
    game_id: i64,
    category_id: i64,

    /// use get/set_state() functions
    state: String,

    // Serialized as seconds-since-epoch
    occurs: i64,

    // message_id is a u64, which sqlx does not want to stick in Sqlite.
    /// Use get/set_message_id() functions
    message_id: Option<String>,
    // TODO: add active_message_id, rename above to scheduling_message_id
}

impl Race {
    fn get_message_id(&self) -> Option<MessageId> {
        match &self.message_id {
            Some(s) => match s.parse::<u64>() {
                Ok(id) => Some(MessageId(id)),
                Err(e) => {
                    warn!("Error parsing message id {}: {}", s, e);
                    None
                }
            },
            None => None,
        }
    }

    // TODO: figure out how to make this accept a string too?
    fn set_message_id(&mut self, id: MessageId) {
        self.message_id = Some(id.to_string());
    }

    fn get_occurs(&self) -> DateTime<Tz> {
        unimplemented!()
    }

    fn set_occurs(&mut self, occurs: DateTime<Tz>) {
        self.occurs = occurs.timestamp();
    }

    fn get_state(&self) -> RaceState {
        RaceState::from_str(&self.state).unwrap()
    }

    fn set_state(&mut self, state: RaceState) {
        self.state = state.to_string();
    }
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

async fn get_upcoming_races(window: Duration, pool: &SqlitePool) -> Vec<Race> {
    let now = Local::now().timestamp();
    let until = (Local::now() + CDuration::from_std(window).unwrap()).timestamp();
    let state = RaceState::SCHEDULED.to_string();
    let q = sqlx::query_as!(
        Race,
        "SELECT * FROM race WHERE state = ? and occurs > ? and occurs < ?",
        state,
        now,
        until
    );
    let mut rows = q.fetch(pool);
    let mut races = vec![];
    while let r = rows.try_next().await {
        match r {
            Ok(mc) => match mc {
                Some(c) => {
                    races.push(c);
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

    races
}

async fn setup_emojis(guild: &Box<GuildCreate>, bot_state: Arc<BotState>) {
    let mut lock = bot_state.emojis.write().await;
    for e in &guild.emojis {
        lock.insert(e.name.to_string(), e.clone());
        debug!("Inserting emoji {}", e.name.to_string());
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
    let desired_roles = vec![
        DesiredRoleBuilder::default()
            .name("active-racer".to_string())
            .color(0xE74C3C)
            .mentionable(true)
            .build()
            .unwrap(),
        DesiredRoleBuilder::default()
            .name("unconfirmed-racer".to_string())
            .color(0xf7c9c4)
            .mentionable(true)
            .build()
            .unwrap(),
    ];

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
    use crate::discord::{
        create_race, get_category, get_game, get_pool, get_upcoming_races, parse_time, update_race,
        Race, RaceState,
    };
    use chrono::{Datelike, Duration as CDuration, Local, NaiveDateTime, Timelike};
    use chrono_tz::US::Eastern;
    use sqlx::SqlitePool;
    use tokio::time::Duration;
    use twilight_model::id::MessageId;

    fn init() {
        let _ = env_logger::builder().is_test(true).try_init();
    }

    async fn initdb(pool: &SqlitePool) {
        let queries = vec![
            "DELETE FROM race",
            "DELETE FROM category",
            "DELETE FROM game",
            "INSERT INTO game (id, name, name_pretty) VALUES (1, 'alttp', 'A Link To The Past')",
            "INSERT INTO category (game_id, name, name_pretty) VALUES (1, 'ms', 'Master Sword')",
            "INSERT INTO category (game_id, name, name_pretty) VALUES (1, 'nmg', 'Any% NMG No S&Q')"
            ];
        for sql in queries {
            let q = sqlx::query(sql);
            q.execute(pool).await;
        }
    }

    #[test]
    fn test_datetime_stuff() {
        init();
        let ndt = NaiveDateTime::parse_from_str("06/09/2021 11:00pm", "%m/%d/%Y %I:%M%P");
        ndt.unwrap();
    }

    #[test]
    fn test_parse_time() {
        init();
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

    // N.B. any test that hits the database needs this annotation. the flavor="multi_thread" part is
    // required to allow get_pool() to resolve, which eventually bottoms out doing something
    // blocking, apparently.
    #[tokio::test(flavor = "multi_thread")]
    async fn test_race_operations() {
        init();
        let pool = get_pool().await.unwrap();
        initdb(&pool).await;
        let g = get_game("alttp", &pool).await.unwrap();
        let c = get_category(&g, "nmg", &pool).await.unwrap();
        let when = parse_time("06/09/2021 11:10pm").unwrap();
        let r = create_race(&g, &c, when, &pool).await;
        assert!(r.is_some());
        let mut race = r.unwrap();
        assert_eq!(race.occurs, when.timestamp());
        assert_eq!(race.category_id, c.id);
        assert_eq!(race.game_id, g.id);
        assert_eq!(race.message_id, None);
        assert_eq!(race.get_state(), RaceState::SCHEDULED);
        let mid = MessageId(u64::MAX);
        race.set_message_id(mid);
        race.set_state(RaceState::ACTIVE);
        update_race(&race, &pool).await.unwrap();

        // it would be reasonable to add a get_race_by_id() kind of message, but I don't think it's
        // actually useful yet.
        let q = sqlx::query_as!(Race, "SELECT * FROM race WHERE id = ?", race.id);
        let race_refreshed = q.fetch_one(&pool).await.unwrap();
        assert_eq!(race, race_refreshed);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_get_races() {
        init();
        let pool = get_pool().await.unwrap();
        initdb(&pool).await;

        let g = get_game("alttp", &pool).await.unwrap();
        let c = get_category(&g, "nmg", &pool).await.unwrap();

        let when = Local::now();
        let later =
            (when + CDuration::from_std(Duration::from_secs(60)).unwrap()).with_timezone(&Eastern);

        // let when = Eastern::now() + CDuration::from_std(Duration::from_secs(60)).unwrap();
        let r = create_race(&g, &c, later, &pool).await;
        assert!(r.is_some());

        let scheduled = get_upcoming_races(Duration::from_secs(120), &pool).await;
        assert_eq!(1, scheduled.len());
    }
}
