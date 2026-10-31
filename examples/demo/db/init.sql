CREATE TABLE stars (
    id          SERIAL PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,
    constellation TEXT NOT NULL,
    distance_ly DOUBLE PRECISION NOT NULL,
    flux        DOUBLE PRECISION NOT NULL,
    magnitude   DOUBLE PRECISION
);

CREATE TABLE observations (
    id          SERIAL PRIMARY KEY,
    star_id     INTEGER NOT NULL REFERENCES stars(id),
    site        TEXT NOT NULL,
    seeing      DOUBLE PRECISION NOT NULL,
    observed_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE inventory (
    id          SERIAL PRIMARY KEY,
    instrument  TEXT NOT NULL UNIQUE,
    available   INTEGER NOT NULL
);

CREATE TABLE events (
    id          SERIAL PRIMARY KEY,
    topic       TEXT NOT NULL,
    payload     JSONB NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

INSERT INTO stars (name, constellation, distance_ly, flux) VALUES
    ('Sirius',     'Canis Major', 8.6,    39.8),
    ('Canopus',    'Carina',      310.0,  1.95),
    ('Arcturus',   'Boötes',      36.7,   1.08),
    ('Vega',       'Lyra',        25.0,   1.00),
    ('Capella',    'Auriga',      42.9,   0.93),
    ('Rigel',      'Orion',       860.0,  0.86),
    ('Procyon',    'Canis Minor', 11.5,   0.70),
    ('Betelgeuse', 'Orion',       640.0,  0.63),
    ('Achernar',   'Eridanus',    139.0,  0.62),
    ('Hadar',      'Centaurus',   390.0,  0.55),
    ('Altair',     'Aquila',      16.7,   0.49),
    ('Acrux',      'Crux',        320.0,  0.44),
    ('Aldebaran',  'Taurus',      65.0,   0.42),
    ('Antares',    'Scorpius',    550.0,  0.37),
    ('Spica',      'Virgo',       250.0,  0.36),
    ('Pollux',     'Gemini',      34.0,   0.33),
    ('Fomalhaut',  'Piscis Austrinus', 25.0, 0.32),
    ('Deneb',      'Cygnus',      2600.0, 0.31),
    ('Mimosa',     'Crux',        280.0,  0.30),
    ('Regulus',    'Leo',         79.0,   0.28);

INSERT INTO inventory (instrument, available) VALUES
    ('main-telescope', 1),
    ('spectrograph', 2),
    ('ccd-camera', 3);

INSERT INTO observations (star_id, site, seeing)
SELECT id, 'paranal', 0.8 FROM stars WHERE id % 3 = 0;
