.PHONY: help all clean test build release lint fmt check-fmt typecheck markdownlint \
	spelling nixie interrogate
.PHONY: test-workflow-contracts


TARGET ?= dbar

CARGO ?= cargo
BUILD_JOBS ?=
RUST_FLAGS ?= -D warnings
CARGO_FLAGS ?= --all-targets --all-features
CLIPPY_FLAGS ?= $(CARGO_FLAGS) -- $(RUST_FLAGS)
TEST_FLAGS ?= $(CARGO_FLAGS)
MDLINT ?= markdownlint-cli2
# `make fmt` and `make check-fmt` call mdtablefix directly. `--git` selects the
# Markdown files Git tracks and `--include-untracked` adds the untracked files
# Git does not ignore, so a new document is formatted before it is staged.
# Both modes need mdtablefix 0.6.0 or later; CI pins the version at the
# install-mdtablefix step.
MDTABLEFIX ?= mdtablefix
MDTABLEFIX_SELECT = --git --include-untracked
MDTABLEFIX_RULES = --wrap --renumber --breaks --ellipsis --fences
NIXIE ?= nixie
WHITAKER ?= whitaker
TYPOS_CONFIG_BUILDER_VERSION ?= v0.1.3
TYPOS_CONFIG_BUILDER = uv tool run --from \
	"git+https://github.com/leynos/typos-config-builder.git@$(TYPOS_CONFIG_BUILDER_VERSION)" \
	typos-config-builder

# The development build standard (concordat rule `rust-build-defaults`):
# the parallel rustc frontend and, on Linux, the mold linker. An assigned
# RUSTFLAGS replaces every `rustflags` table in .cargo/config.toml, so each
# recipe that sets it composes these onto any inherited value (CI's
# setup-rust exports one), except coverage, which stays on LLVM and the
# platform linker.
BUILD_HOST_OS := $(shell uname -s)
STANDARD_RUSTFLAGS := -Zthreads=8$(if $(filter Linux,$(BUILD_HOST_OS)), -Clink-arg=-fuse-ld=mold)

build: target/debug/$(TARGET) ## Build debug binary
release: target/release/$(TARGET) ## Build release binary

UV ?= uv
UV_ENV ?=
# The shared CV-005 contract (leynos/shared-actions, `cv005-contracts`) is run
# from a pinned commit: a fix to the rule reaches this repository as a reviewed
# bump of the pin, not as a silent upgrade. `.github/cv005.toml` holds the
# parameters only.
CV005_CONTRACTS_REF ?= cabf105ae230e3759cf77b1c2d1d73ea0b67e9a9
CV005_CONTRACTS = $(UV_ENV) $(UV) tool run --python 3.13 \
	--from 'git+https://github.com/leynos/shared-actions@$(CV005_CONTRACTS_REF)\#subdirectory=packages/cv005-contracts' \
	cv005-contracts

all: check-fmt lint test test-workflow-contracts ## Perform a comprehensive check of code

clean: ## Remove build artifacts
	$(CARGO) clean
	rm -f .typos-oxendict-base.json .typos-oxendict-base.toml

test: ## Run tests with warnings treated as errors
	RUSTFLAGS="$${RUSTFLAGS:+$$RUSTFLAGS }$(RUST_FLAGS) $(STANDARD_RUSTFLAGS)" $(CARGO) test $(TEST_FLAGS) $(BUILD_JOBS)

target/%/$(TARGET): ## Build binary in debug or release mode
	$(if $(findstring release,$(@)),RUSTFLAGS="$${RUSTFLAGS-}",RUSTFLAGS="$${RUSTFLAGS:+$$RUSTFLAGS }$(STANDARD_RUSTFLAGS)") $(CARGO) build $(BUILD_JOBS) $(if $(findstring release,$(@)),--release) --bin $(TARGET)

# Interrogate measures docstring presence over every Python file the repository
# owns. Add a new Python directory to INTERROGATE_TARGETS. No `--python` pin: a
# managed interpreter would be downloaded, and a PyO3 build could link it.
INTERROGATE_VERSION ?= 1.7.0
INTERROGATE_TARGETS ?= tests
# Level 2 prints a MISSED row per undocumented definition, which the
# `interrogate` target fails on: the rounded percentage alone passes a lone gap.
INTERROGATE_FLAGS ?= -vv --fail-under 100
INTERROGATE = $(UV_ENV) $(UV) tool run \
	--from 'interrogate==$(INTERROGATE_VERSION)' interrogate

# Interrogate rounds its percentage to one decimal, so `--fail-under 100` alone
# passes a lone gap in a large repository; fail on any MISSED row as well.
interrogate: ## Enforce complete Python docstring coverage
	@report="$$($(INTERROGATE) $(INTERROGATE_FLAGS) $(INTERROGATE_TARGETS))"; \
	  status=$$?; \
	  printf '%s\n' "$$report"; \
	  test "$$status" -eq 0 || exit "$$status"; \
	  if printf '%s\n' "$$report" | grep -q '| *MISSED |'; then \
	    printf '%s\n' 'interrogate: undocumented definitions remain; see the MISSED rows above' >&2; \
	    exit 1; \
	  fi

# Whitaker runs under the toolchain its installer manages, so it takes none of
# the development standard: its literal `RUSTFLAGS` displaces the `rustflags`
# tables (no frontend flag, no `mold`) and inherits nothing from the caller. It
# also runs from the checkout's parent with an absolute manifest path, so Cargo
# never discovers this checkout's `.cargo/config.toml`, whose Cranelift profile
# the installer-managed toolchain does not carry.
lint: interrogate ## Run Clippy and the Whitaker Dylint suite with warnings denied
	RUSTDOCFLAGS="$(RUSTDOC_FLAGS)" RUSTFLAGS="$${RUSTFLAGS:+$$RUSTFLAGS }$(STANDARD_RUSTFLAGS)" $(CARGO) doc --no-deps
	RUSTFLAGS="$${RUSTFLAGS:+$$RUSTFLAGS }$(STANDARD_RUSTFLAGS)" $(CARGO) clippy $(CLIPPY_FLAGS)
	cd "$(dir $(CURDIR))" && RUSTFLAGS="-D warnings" $(WHITAKER) --manifest-path "$(CURDIR)/Cargo.toml" --all -- $(CARGO_FLAGS)
	+$(MAKE) spelling

fmt: ## Format Rust and Markdown sources
	$(CARGO) fmt --all
	$(MDTABLEFIX) --in-place $(MDTABLEFIX_SELECT) $(MDTABLEFIX_RULES)
	$(MDLINT) --fix "**/*.md"

check-fmt: ## Verify formatting
	$(CARGO) fmt --all -- --check
	$(MDTABLEFIX) --check $(MDTABLEFIX_SELECT) $(MDTABLEFIX_RULES)

typecheck: ## Type-check every target with every feature
	RUSTFLAGS="$${RUSTFLAGS:+$$RUSTFLAGS }$(RUST_FLAGS) $(STANDARD_RUSTFLAGS)" $(CARGO) check $(CARGO_FLAGS) $(BUILD_JOBS)

markdownlint: ## Lint Markdown files
	$(MDLINT) '**/*.md'
	+$(MAKE) spelling

spelling: ## Enforce en-GB-oxendict spelling
	$(TYPOS_CONFIG_BUILDER) gate --repository .

nixie: ## Validate Mermaid diagrams
	$(NIXIE) --no-sandbox

help: ## Show available targets
	@grep -E '^[a-zA-Z_-]+:.*?##' $(MAKEFILE_LIST) | \
	awk 'BEGIN {FS=":"; printf "Available targets:\n"} {printf "  %-20s %s\n", $$1, $$2}'

test-workflow-contracts: ## Validate the CodeScene coverage workflow contract (CV-005)
	$(CV005_CONTRACTS) check --repository .
	$(UV_ENV) $(UV) run --python 3.13 --with 'pytest>=8,<10' --with 'pyyaml>=6,<7' pytest tests/workflow_contracts -q
