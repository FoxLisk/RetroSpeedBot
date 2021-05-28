# Building

This is an IntelliJ Idea project. You don't necessarily need to use that but it'll make your life easier.

You'll need access to the App Token, and then to set it in your environment as `DISCORD_TOKEN="<the token>"`.
You can get this by asking me (FoxLisk) and getting
added to the discord developer team. I suppose you could also use this code to run a different bot with a different
token, if that were your idea of a good time.

I have the following environment variables set:

```
RUST_BACKTRACE=full
RUST_LOG=retro_speed_bot=Debug
```

For debugging. `RUST_LOG` is for the `env_logger` library.

# Basic Structure

`build.rs` is a build script that is called at the beginning of compilation. This is required to set up database state.
See [sqlx migration documentation](https://docs.rs/sqlx/0.5.2/sqlx/migrate/struct.Migrator.html) or the [more-useful examples](
https://github.com/launchbadge/sqlx/tree/master/examples/sqlite/todos). It is current practice to wipe and rebuild the db
whenever I feel like it in testing.

`main.rs` is a very thin hub. It should do as little as possible to set tokio threads working.

`discord.rs` is handling all of the discord bot stuff right now. This should be split up into modules probably but I
haven't bothered yet. The main things are:

1. `run_bot()` - the entry point. This sets up various state and then starts the main event handling loops
1. `cron()` - this is a stub, but the point is to, every $DURATION, check if any stuff needs to be handled - if there's
   a race coming up that we should assign roles for, if people need to be pinged, etc.
1. `handle_events()` - this is the discord-event handler. It eventually dispatches into `handle_event` where most of the
   important stuff is handling `MessageCreate`s. 
1. `models.rs` - currently `Game`, `Category`, and `Race`. These have
   some CRUD methods. Some of this is macro-generated by things in `procm/src/lib.rs` but not much. macros are hard.

   
Most incoming user requests go through `handle_events()`, get dispatched to an interim function such as
`list_categories`, which handles command parsing, then grabs the data via `get_categories`, and then formats them
and sends them back to discord. the point here being to separate out the DB access from the rest.
   
# Running tests:

* Set `DATABASE_URL` to something other than `real_db.db3` in your environment
* Run `test -- --test-threads=1`.
* making a reasonable test harness is TODO; it's currently a mess down there.

# TODOs:

## critical path TODOs:

 * get it running on linux (hopefully (*gulp*) this is easy)
 * races should have a notes field - stuff like "for new runners".
 * a command to delete a race

## misc TODOs - not any special order:

 * we should handle some kind of discord state cleanup, eventually, i think.
   for now if the bot crashes or restarts or whatever, roles will get out of sync, etc.
   * maybe just some kind of message that's like "react here to cleanse yourself of roles"
 * handle database migrations much better (see note in build.rs)
 * try to reduce dependencies? release builds take forever.
   * could just use debug builds tbh, performance isn't gonna matter
   * might be able to toggle off a bunch of features anyway
 * Handle any kind of internet outage? idk if it's worth it vs just restarting *shrug*
 * Rate limit the bot
 * ORM stuff is looking more and more desireable
   * OMFG look into this!!! https://docs.rs/ormx/0.7.0/ormx/
   * the documentation is - of course! - a total joke, but maybe very useful.
 * bot commands to manipulate games/categories instead of "tell a dev and get it added to the build script"
 * 80 trillion unit tests, ideally
 * this list itself should be in github maybe
 * racetime.gg integration would be cool
 * scheduled races would be cool
 * can we move people into appropriate voice chats?
   * should we?
