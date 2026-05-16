REGISTRY ?= ghcr.io/marpaia/sbol-db

GIT_TAG   := $(shell git describe --tags --exact-match --dirty 2>/dev/null)
GIT_HASH  := $(shell git rev-parse --short HEAD 2>/dev/null)
GIT_DIRTY := $(shell git diff --quiet HEAD 2>/dev/null || echo -dirty)
VERSION   := $(or $(GIT_TAG),$(GIT_HASH)$(GIT_DIRTY))

IMAGE ?= $(REGISTRY):$(VERSION)

.PHONY: psql container

psql:
	docker compose exec -e PGPASSWORD=sbol postgres psql -U sbol -d sbol

container:
	docker buildx build --load --tag $(IMAGE) .
