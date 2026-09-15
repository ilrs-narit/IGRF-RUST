################################################################################
#
# igrf-app
#
################################################################################

# Prototype source mode: build straight from a local checkout of this
# repository. The default is this checkout, since the external tree lives in
# the repository's buildroot/ directory; to build a different one, set
# IGRF_APP_OVERRIDE_SRCDIR in local.mk (uncommitted, next to the .config):
#
#   IGRF_APP_OVERRIDE_SRCDIR = /abs/path/to/IGRF-RUST
#
# pkg-generic.mk maps the local site method onto _OVERRIDE_SRCDIR, rsyncs the
# checkout into the build dir, and skips the download step entirely. Local
# mode therefore needs no .hash and this package deliberately ships none.
#
# The default is not a convenience. For a local site method pkg-generic.mk
# assigns _SITE to _OVERRIDE_SRCDIR when the latter is empty, so leaving the
# override unset on a _SITE that is itself _OVERRIDE_SRCDIR kills the build at
# parse time with "Recursive variable ... references itself", naming a
# variable the .mk never defines. The ?= keeps command-line and local.mk
# overrides working.
#
# The version is overridden to "custom" by pkg-generic in this mode; the
# value below is the igrf-app crate version for the release path.
IGRF_APP_VERSION = 0.5.0
IGRF_APP_SITE_METHOD = local
IGRF_APP_OVERRIDE_SRCDIR ?= $(abspath $(BR2_EXTERNAL_IGRF_PATH)/..)
IGRF_APP_SITE = $(IGRF_APP_OVERRIDE_SRCDIR)

# Release path (NOT in use for the prototype): a pinned tarball plus a real
# .hash covering it. The cargo infrastructure runs `cargo vendor` at download
# time and caches the dependencies inside the tarball in DL_DIR, so the hash
# covers app sources and vendored crates together. Never fabricate a hash for
# a tarball that was not actually downloaded.
#
# IGRF_APP_SITE = $(call github,ilrs-narit,IGRF-RUST,v$(IGRF_APP_VERSION))
# (drop the two SITE_METHOD/_SITE local lines above when using this)

# The cargo-package install step hardcodes `--path ./` and appends
# _CARGO_INSTALL_OPTS after it; cargo rejects a repeated --path ("the
# argument '--path <PATH>' cannot be used multiple times"), so passing
# `--path igrf-app` in _CARGO_INSTALL_OPTS cannot work. The repository root
# Cargo.toml is a virtual workspace (members igrf-core, igrf-io, igrf-app)
# and `cargo install --path ./` there would install nothing, so the package
# root is pointed at the member crate instead: from igrf-app/ cargo
# discovers the workspace root and Cargo.lock upward, `--path ./` resolves to
# the member crate, and its [[bin]] installs as /usr/bin/igrf-app.
IGRF_APP_SUBDIR = igrf-app

# rfd's gtk3 backend (map-grid file dialog) needs GTK3 in staging to build
# (gtk-sys uses pkg-config) and on the target at runtime.
IGRF_APP_DEPENDENCIES = libgtk3

# With the local site method there is no download step, so the cargo
# infrastructure's vendoring (a download post-process) never runs, and its
# hardcoded `cargo build/install --offline --locked` would find an empty
# cargo cache. Populate Buildroot's cargo home with the locked dependencies
# before configure; the release tarball path vendors at download time and
# does not need this hook.
define IGRF_APP_FETCH_CARGO_DEPS
	cd $(IGRF_APP_SRCDIR) && \
		$(HOST_MAKE_ENV) \
		CARGO_HOME=$(BR_CARGO_HOME) \
		cargo fetch --locked --target $(RUSTC_TARGET_NAME)
endef
IGRF_APP_PRE_CONFIGURE_HOOKS += IGRF_APP_FETCH_CARGO_DEPS

# Seed source for the first-boot unit owned by issue #21, which copies this
# into /data/igrf/SystemConfig.json when none exists yet.
define IGRF_APP_INSTALL_EXAMPLE_CONFIG
	$(INSTALL) -D -m 0644 $(IGRF_APP_DIR)/SystemConfig.example.json \
		$(TARGET_DIR)/usr/share/igrf/SystemConfig.example.json
endef
IGRF_APP_POST_INSTALL_TARGET_HOOKS += IGRF_APP_INSTALL_EXAMPLE_CONFIG

# The .wants symlink is relative: no absolute host paths land in the target
# rootfs, and the link still resolves inside the image.
define IGRF_APP_INSTALL_INIT_SYSTEMD
	$(INSTALL) -D -m 0644 $(IGRF_APP_PKGDIR)/igrf-app.service \
		$(TARGET_DIR)/usr/lib/systemd/system/igrf-app.service
	mkdir -p $(TARGET_DIR)/etc/systemd/system/multi-user.target.wants
	ln -sf ../../../../usr/lib/systemd/system/igrf-app.service \
		$(TARGET_DIR)/etc/systemd/system/multi-user.target.wants/igrf-app.service
endef

$(eval $(cargo-package))
