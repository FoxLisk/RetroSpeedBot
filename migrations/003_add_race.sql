CREATE TABLE IF NOT EXISTS race
(
    id          INTEGER PRIMARY KEY,
    game_id     INTEGER NOT NULL,
    category_id INTEGER NOT NULL,
    occurs      INTEGER NOT NULL,
    message_id  INTEGER NULL,

    FOREIGN KEY(game_id) REFERENCES game(id),
    FOREIGN KEY(category_id) REFERENCES category(id),
    CONSTRAINT game_cat_time UNIQUE (game_id, category_id, occurs)
);