# Development tasks for the sbol-db Python client, run through uv.
#
#   make install   # sync the uv environment (deps + dev group)
#   make test      # unit tests only (no server needed)
#   make e2e       # build the sbol-db server, then run the full suite
#   make lint      # isort + black + flake8 + mypy
#
# The e2e target builds a fresh sbol-db binary and points the test fixtures at
# it, so tests never run against a stale build. Set SBOL_DB_TEST_BACKENDS to add
# backends, e.g. `make e2e SBOL_DB_TEST_BACKENDS=sqlite,postgres` (Postgres
# needs `docker compose up -d postgres` first).

REPO_ROOT := $(abspath ../..)
SBOL_DB_BIN := $(REPO_ROOT)/target/debug/sbol-db

.PHONY: install test e2e lint build check publish clean

install:
	uv sync

test:
	uv run pytest -m "not e2e"

e2e:
	cargo build --manifest-path $(REPO_ROOT)/Cargo.toml -p sbol-db
	SBOL_DB_BIN=$(SBOL_DB_BIN) uv run pytest

lint:
	uv run isort --check-only --profile black --line-length 120 src tests
	uv run black --check --line-length 120 src tests
	uv run flake8 --max-line-length 120 --extend-ignore E203,W503 src tests
	uv run mypy src

build: clean
	uv build

check: build
	uvx twine check dist/*

publish: check
	uv publish dist/*

clean:
	rm -rf dist
