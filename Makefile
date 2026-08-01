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

HA_CHAOS_SEED ?= 0x5b01db0000000001
HA_CHAOS_TRACE ?= target/sbol-db-ha-chaos.json
HA_PROCESS_ARTIFACT_ROOT ?= target/ha-runs/process-$(shell date -u +%Y%m%dT%H%M%SZ)

.PHONY: psql container container/test-faiss container/test-sbol-test-suite ha/chaos-sbol-test-suite ha/test-process ha/process-sbol-test-suite model/bge-small

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

ha/chaos-sbol-test-suite:
	@test -n "$(SBOL_TEST_SUITE_ROOT)" || (echo "set SBOL_TEST_SUITE_ROOT to a pinned SBOLTestSuite checkout" >&2; exit 2)
	cargo run -p sbol-db-ha-sim -- \
		--corpus-root "$(SBOL_TEST_SUITE_ROOT)" \
		--seed "$(HA_CHAOS_SEED)" \
		--trace "$(HA_CHAOS_TRACE)"

ha/test-process:
	cargo test -p sbol-db-ha-test --test process_stack

ha/process-sbol-test-suite:
	@test -n "$(SBOL_TEST_SUITE_ROOT)" || (echo "set SBOL_TEST_SUITE_ROOT to a pinned SBOLTestSuite checkout" >&2; exit 2)
	cargo build -p sbol-db-ha-test --bins
	cargo run -p sbol-db-ha-test --bin sbol-db-ha-runner -- \
		--node-binary target/debug/sbol-db-ha-node \
		--corpus-root "$(SBOL_TEST_SUITE_ROOT)" \
		--seed "$(HA_CHAOS_SEED)" \
		--artifact-root "$(HA_PROCESS_ARTIFACT_ROOT)"

model/bge-small:
	bash docker/fetch-builtin-bge-small-model.sh $(BUILTIN_BGE_SMALL_DIR)
	@echo "BGE-small bundle ready at $(BUILTIN_BGE_SMALL_DIR) (auto-discovered by source builds)"
