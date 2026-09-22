PRAGMA foreign_keys=OFF;
BEGIN TRANSACTION;
CREATE TABLE cards (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    title TEXT NOT NULL,
    tag TEXT,
    description TEXT NOT NULL DEFAULT '',
    "column" TEXT NOT NULL DEFAULT 'todo' CHECK ("column" IN ('todo','doing','review','done')),
    owner TEXT,
    due TEXT,
    gh_ref INTEGER,
    created_at INTEGER NOT NULL,
    column_since INTEGER NOT NULL,
    blocked TEXT,
    position INTEGER NOT NULL DEFAULT 0
, reviewer TEXT);
INSERT INTO cards VALUES(1,'login form rejects long names','widgets','Done = a 64-character name logs in','doing','bot-1',NULL,12,1767261660,1767262140,NULL,0,NULL);
INSERT INTO cards VALUES(2,'renew the domain',NULL,'','todo',NULL,NULL,NULL,1767261720,1767261720,'#1',1,NULL);
CREATE TABLE checklist (
    card_id INTEGER NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
    idx INTEGER NOT NULL,
    text TEXT NOT NULL,
    done INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (card_id, idx)
);
INSERT INTO checklist VALUES(1,1,'write the test',1);
INSERT INTO checklist VALUES(1,2,'ship it',0);
CREATE TABLE events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    card_id INTEGER NOT NULL REFERENCES cards(id) ON DELETE CASCADE,
    ts INTEGER NOT NULL,
    actor TEXT NOT NULL,
    kind TEXT NOT NULL,
    text TEXT NOT NULL DEFAULT ''
);
INSERT INTO events VALUES(1,1,1767261660,'lead','created','');
INSERT INTO events VALUES(2,2,1767261720,'pat','created','');
INSERT INTO events VALUES(4,1,1767261840,'bot-1','taken','');
INSERT INTO events VALUES(5,1,1767261900,'bot-1','note','half way: the test fails as expected');
INSERT INTO events VALUES(6,1,1767261960,'bot-1','check','[x] write the test');
INSERT INTO events VALUES(7,1,1767262020,'bot-1','moved','doing -> review');
INSERT INTO events VALUES(8,1,1767262080,'rev-1','reviewing','');
INSERT INTO events VALUES(9,1,1767262140,'rev-1','moved','review -> doing');
INSERT INTO events VALUES(10,1,1767262140,'rev-1','returned','the long-name case still fails');
INSERT INTO events VALUES(11,2,1767262200,'pat','blocked','by #1');
CREATE TABLE github_snapshot (
    key INTEGER PRIMARY KEY CHECK (key = 1),
    fetched_at INTEGER NOT NULL DEFAULT 0,
    json TEXT,
    error TEXT,
    fails INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE board_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ts INTEGER NOT NULL,
    actor TEXT NOT NULL,
    kind TEXT NOT NULL,
    text TEXT NOT NULL DEFAULT ''
);
INSERT INTO board_events VALUES(1,1767262260,'lead','wip','wip 3 -> 4');
INSERT INTO board_events VALUES(2,1767262320,'lead','delete','deleted #3 "install guide"');
CREATE TABLE config (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
INSERT INTO config VALUES('wip','4');
INSERT INTO sqlite_sequence VALUES('cards',3);
INSERT INTO sqlite_sequence VALUES('events',11);
INSERT INTO sqlite_sequence VALUES('board_events',2);
CREATE INDEX cards_column ON cards("column");
CREATE INDEX events_card ON events(card_id, id);
COMMIT;
