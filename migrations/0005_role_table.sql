-- Normalise roles: replace the free-text role column with a FK to a roles table.

CREATE TABLE roles (
    id   SMALLINT PRIMARY KEY GENERATED ALWAYS AS IDENTITY,
    name TEXT     NOT NULL UNIQUE
);

-- Seed the two initial roles (subscriber = 1, admin = 2)
INSERT INTO roles (name) VALUES ('subscriber'), ('admin');

-- Add the FK column; nullable initially so we can back-fill existing rows.
ALTER TABLE users ADD COLUMN role_id SMALLINT REFERENCES roles(id);

-- Back-fill from the text column written by migration 0004.
UPDATE users
SET role_id = r.id
FROM roles r
WHERE r.name = users.role;

-- Enforce NOT NULL now that every row has a value.
ALTER TABLE users ALTER COLUMN role_id SET NOT NULL;

-- The old text column is no longer needed.
ALTER TABLE users DROP COLUMN role;
