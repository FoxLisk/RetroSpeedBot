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
https://github.com/launchbadge/sqlx/tree/master/examples/sqlite/todos).

`main.rs` is a very thin hub. It should do as little as possible to set tokio threads working.

`discord.rs` is handling all of the discord bot stuff right now. This should be split up into modules probably but I
haven't bothered yet. The main things are:

1. `run_bot()` - the entry point. This sets up various state and then starts the main event handling loops
1. `cron()` - this is a stub, but the point is to, every $DURATION, check if any stuff needs to be handled - if there's
   a race coming up that we should assign roles for, if people need to be pinged, etc.
1. `handle_events()` - this is the discord-event handler. It eventually dispatches into `handle_event` where most of the
   important stuff is handling `MessageCreate`s. 
1. The models - these should have their own module, but they're currently `Game`, `Category`, and `Race`. These have
   some CRUD methods. The CRUD methods should honestly be macro-generated - but I want to get something working before
   I indulge my habit of toying around with language features _too_ much.

# TODOs:

## critical path TODOs:

 * some sort of way to sync the messages w/ the races
   * perhaps the race table should have a channel/message id of the associated message?
 * actually do stuff when race time is getting closer
   * probably have some every-minute or every-5-minutes thing that checks if there's a race
     coming up, and if so finds it in the scheduling channel and does stuff with the reacts
 * get it running on linux (hopefully (*gulp*) this is easy)
 * races should have a notes field - stuff like "for new runners".
 * a command to delete a race
  

## misc TODOs - not any special order:

 * Rate limit the bot
 * add a !commands command, or similar
 * pre-populate race reactions
 * i kinda think sending newly created races, possibly fully hydrated, off to some like mpsc-based
   handler might be the way of the hero?
 * ORM stuff is looking more and more desireable
 * keeping some stuff in memory would probably be cool, although N ~= 1 for a long time so it shouldn't matter. Things
   like races should be cheap enough to store in memory and then mega fast to iterate over, if we wanted.
 * scheduled races would be cool
 * bot commands to manipulate games/categories instead of "tell a dev and get it added to the build script"
 * 80 trillion unit tests, ideally
 * this list itself should be in github maybe

