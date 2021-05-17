CREATE TABLE IF NOT EXISTS category
(
    id          INTEGER PRIMARY KEY NOT NULL,
    game_id     INTEGER NOT NULL,
    name        TEXT UNIQUE NOT NULL,
    name_pretty TEXT UNIQUE NOT NULL,
    FOREIGN KEY(game_id) REFERENCES game(id)
);