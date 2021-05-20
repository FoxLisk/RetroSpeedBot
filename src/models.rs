use chrono::DateTime;
use procm::model;
use twilight_model::id::MessageId;

use chrono_tz::Tz;
use sqlx::SqlitePool;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

#[derive(Debug, PartialEq)]
pub(crate) enum RaceState {
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
pub(crate) struct ParseError;
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


// TODO: hmmm... how to handle FKs? i think *for now* it's fine to just do stuff top down.
//       probably eventually we want some kind of hydration


// Would love to have a real ORM... oh well
// it's probably actually better to have id: Option<i64> so we can create models that aren't
// DB-backed
model! {
#[derive(Debug, PartialEq)]
pub(crate) struct Game {
    pub(crate)   id: i64,
    pub(crate)   name: String,
    pub(crate)  name_pretty: String,
}
}

model! {
#[derive(Debug, PartialEq)]
pub(crate) struct Category {
    pub(crate)   id: i64,
    pub(crate)   game_id: i64,
    pub(crate)   name: String,
    pub(crate)   name_pretty: String,
}
}

model! {
#[derive(Debug, PartialEq)]
pub(crate) struct Race {
    pub(crate) id: i64,
    // N.B. game_id is not strictly necessary in this struct
    pub(crate) game_id: i64,
    pub(crate) category_id: i64,

    /// use get/set_state() functions
    pub(crate) state: String,

    // Serialized as seconds-since-epoch
    pub(crate) occurs: i64,

    // message_id is a u64, which sqlx does not want to stick in Sqlite.
    /// Use get/set_scheduling_message_id() functions
    pub(crate) scheduling_message_id: Option<String>,

    /// use get/set_active_message_id() functions
    pub(crate) active_message_id: Option<String>,
}
}

impl Race {

    /// Creates a new race with the initial parameters. Does not persist.
    /// State will be set to SCHEDULED.
    pub(crate) fn new(id: i64, game_id: i64, category_id: i64, occurs: DateTime<Tz>) -> Self {
        let mut r = Race {
            id, game_id, category_id, state: "".to_string(), occurs: 0, scheduling_message_id: None, active_message_id: None,
        };
        r.set_state(RaceState::SCHEDULED);
        r.set_occurs(occurs);
        r
    }

    pub(crate) fn get_scheduling_message_id(&self) -> Option<MessageId> {
        match &self.scheduling_message_id {
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
    pub(crate) fn set_scheduling_message_id(&mut self, id: MessageId) {
        self.scheduling_message_id = Some(id.to_string());
    }

    pub(crate) fn get_active_message_id(&self) -> Option<MessageId> {
        match &self.active_message_id {
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
    pub(crate) fn set_active_message_id(&mut self, id: MessageId) {
        self.active_message_id = Some(id.to_string());
    }

    pub(crate) fn get_occurs(&self) -> DateTime<Tz> {
        unimplemented!()
    }

    pub(crate) fn set_occurs(&mut self, occurs: DateTime<Tz>) {
        self.occurs = occurs.timestamp();
    }

    pub(crate) fn get_state(&self) -> RaceState {
        RaceState::from_str(&self.state).unwrap()
    }

    pub(crate) fn set_state(&mut self, state: RaceState) {
        self.state = state.to_string();
    }
}

impl Display for Race {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        // TODO: hydrate game/cat and print them in here?
        write!(f, "Race #{}", self.id)
    }
}