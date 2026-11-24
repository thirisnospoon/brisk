# BriskAPI: a web API from a visual model

Status: idea, not started. Work on it begins after the basics listed in the README
roadmap (source roots, Git hunks).

Goal: describe entities and their relations in a small visual editor, pick a preset, and
get a working FastAPI application with a sound architecture in a couple of clicks. Then
keep editing the model visually, or write code by hand, or both, without one breaking the
other.

## What exists elsewhere

- JHipster (Java): entities and relations in JDL Studio, a full application generated from
  them (models, REST, repositories, security, tests). Popular for years.
- Amplication (Node.js): visual entities, a NestJS backend generated. It later moved away
  from that product. Generation is used on day one; after that people live in the code and
  the visual tool stops being opened.
- Python: project templates (full-stack-fastapi-template), generators from an OpenAPI
  spec, CRUD router libraries. No visual entity-to-application tool, and none inside an
  IDE. PyCharm has nothing like it.

## The problem to solve: hand edits

Generators break down when the user edits generated code: regenerating either overwrites
the edits or produces conflicts. A "locked" mode that forbids edits only postpones this.

BriskAPI uses the generation gap pattern instead:

- Generated code lives in its own layer (`app/_generated/`): base classes for models,
  schemas, repositories, services and routers. It carries a header saying it is generated
  and is rewritten on every change of the model.
- User code lives next to it and subclasses or wraps the generated classes (`app/users/`
  with `class UserService(UserServiceBase)`). BriskAPI creates these files once, as stubs,
  and never touches them again.
- So the model can be edited visually at any time and hand-written logic survives. There
  is no lock to toggle. Leaving BriskAPI means deleting the model file; the code stays
  valid.

## The model editor is also for existing projects

The same editor reads existing SQLAlchemy (later Django) models and shows them as an
entity-relationship diagram. Adding a field or a relation there edits the model class and
the Pydantic schemas and creates an Alembic migration. With the Database tool window it
also shows the data behind an entity. This is what makes it useful after day one, and no
Python IDE has it.

## First version

- FastAPI only.
- Entities with fields (common types, nullable, unique, default, indexed) and relations
  (one-to-many, many-to-many), on a canvas with drag to connect.
- Presets: Users and authentication, Blog, Shop, Empty.
- Generated stack: FastAPI, SQLAlchemy 2 (async), Alembic, Pydantic v2, repositories and
  services, JWT authentication with hashed passwords, settings from the environment,
  pytest tests for every router, `docker-compose.yml` with PostgreSQL.
- The model is stored in the project as `briskapi.toml` (readable, diff-friendly).
- A local PostgreSQL started by Brisk (below) so the result runs without Docker.

Later: Flask and Django, reading existing models, more relation options, OpenAPI import.

## Databases with one click

Two pieces, independent of BriskAPI but used by it:

- Services from `docker-compose.yml`: start, stop and see logs of Compose services in the
  Services tool window when Docker is installed. Most real projects describe their
  database this way.
- Local PostgreSQL without Docker: Brisk downloads the PostgreSQL binaries (a zip, no
  installer, no Windows service, no administrator rights), runs `initdb` and `pg_ctl` with
  the data in the project's `.brisk` folder on a free port, adds a data source to the
  Database tool window and writes `DATABASE_URL` to `.env`. Stopping is `pg_ctl stop`;
  removing is deleting the folder.

Open questions: matching the production PostgreSQL version, extensions such as PostGIS,
whether the server stops when Brisk closes, the download size. Redis has no maintained
native Windows build, so a full environment without Docker needs the containers module
([containers.md](containers.md)).

## Risks

- Scope: this is a product direction, not a feature.
- The quality of the generated architecture decides everything. One stack, done well,
  before any others.
- The name must not suggest an official FastAPI product.
