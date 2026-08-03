REGISTRY ?= ghcr.io/marpaia/sbol-db
BUILTIN_BGE_SMALL_DIR ?= $(HOME)/.cache/sbol-db/models/bge-small-en-v1.5-onnx-q-5239827

# bindgen / pg_query SDK isysroot is set via .cargo/config.toml using
# the target-specific BINDGEN_EXTRA_CLANG_ARGS_<triple> hook, so bare
# `cargo build` works on macOS without extra shell setup. Override at
# the shell level if your SDK lives outside the default Xcode path.

GIT_TAG   := $(shell git describe --tags --exact-match --dirty 2>/dev/null)
GIT_HASH  := $(shell git rev-parse --short HEAD 2>/dev/null)
GIT_DIRTY := $(shell git diff --quiet HEAD 2>/dev/null || echo -dirty)
VERSION   := $(or $(GIT_TAG),$(GIT_HASH)$(GIT_DIRTY))

IMAGE ?= $(REGISTRY):$(VERSION)

.PHONY: psql container container/test-faiss container/test-sbol-test-suite model/bge-small \
	fly/render fly/bootstrap fly/init-volume fly/build fly/predeploy-backup fly/deploy fly/verify fly/seed fly/set-sole-admin

psql:
	docker compose exec -e PGPASSWORD=sbol postgres psql -U sbol -d sbol

container:
	docker buildx build --load --tag $(IMAGE) .

container/test-faiss:
	docker buildx build --load --target faiss-test --tag $(IMAGE)-faiss-test .
	docker buildx build --load --tag $(IMAGE) .
	docker/test-faiss-container.sh $(IMAGE)

container/test-sbol-test-suite:
	@test -n "$(SBOL_TEST_SUITE_ROOT)" || (echo "set SBOL_TEST_SUITE_ROOT to a pinned SBOLTestSuite checkout" >&2; exit 2)
	SBOL_DB_SBOL_TEST_SUITE_ROOT="$(SBOL_TEST_SUITE_ROOT)" \
	SBOL_DB_TEST_SUITE_BGE_ENABLED="$(SBOL_DB_TEST_SUITE_BGE_ENABLED)" \
		docker/test-sbol-test-suite-container.sh $(IMAGE)

model/bge-small:
	bash docker/fetch-builtin-bge-small-model.sh $(BUILTIN_BGE_SMALL_DIR)
	@echo "BGE-small bundle ready at $(BUILTIN_BGE_SMALL_DIR) (auto-discovered by source builds)"

fly/render:
	deploy/fly/render.sh

fly/bootstrap:
	deploy/fly/bootstrap.sh

fly/init-volume:
	deploy/fly/init-volume.sh

fly/build:
	deploy/fly/build.sh

fly/predeploy-backup:
	deploy/fly/predeploy-backup.sh $(FLY_IMAGE)

fly/deploy:
	deploy/fly/deploy.sh $(FLY_IMAGE)

fly/verify:
	deploy/fly/verify.sh

fly/seed:
	deploy/fly/seed.sh create

fly/set-sole-admin:
	deploy/fly/set-sole-admin.sh "$(FLY_IMAGE)" "$(SBOL_DB_ADMIN_USERNAME)" "$(SBOL_DB_ADMIN_EMAIL)"
