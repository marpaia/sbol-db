REGISTRY ?= ghcr.io/marpaia/sbol-db

# bindgen / pg_query SDK isysroot is set via .cargo/config.toml using
# the target-specific BINDGEN_EXTRA_CLANG_ARGS_<triple> hook, so bare
# `cargo build` works on macOS without extra shell setup. Override at
# the shell level if your SDK lives outside the default Xcode path.

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
