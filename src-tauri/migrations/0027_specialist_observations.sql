-- Read-only SpecialistBots observations. Never an execution permit.
CREATE TABLE IF NOT EXISTS specialist_projects (
 project TEXT PRIMARY KEY NOT NULL, epoch INTEGER NOT NULL CHECK(epoch >= 0)
);
CREATE TABLE IF NOT EXISTS specialist_bindings (
 id TEXT PRIMARY KEY NOT NULL, project TEXT NOT NULL, epoch INTEGER NOT NULL,
 payload TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS specialist_bindings_project ON specialist_bindings(project);
CREATE TABLE IF NOT EXISTS specialist_observations (
 id TEXT PRIMARY KEY NOT NULL, binding_id TEXT NOT NULL, payload TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS specialist_observations_binding ON specialist_observations(binding_id);
