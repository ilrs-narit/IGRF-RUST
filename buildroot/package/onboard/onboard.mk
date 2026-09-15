################################################################################
#
# onboard
#
################################################################################

ONBOARD_VERSION = 1.4.4-1
ONBOARD_SITE = $(call github,onboard-osk,onboard,$(ONBOARD_VERSION))
ONBOARD_LICENSE = GPL-3.0+
ONBOARD_LICENSE_FILES = COPYING
ONBOARD_SETUP_TYPE = setuptools
ONBOARD_DEPENDENCIES = host-pkgconf host-intltool host-gettext \
	host-python-packaging host-python-distutils-extra host-dconf \
	python3 python-pycairo dbus-python python-gobject gobject-introspection \
	librsvg libgtk3 xlib_libX11 xlib_libXi xlib_libXtst xlib_libxkbfile \
	libcanberra gsettings-desktop-schemas dconf hunspell iso-codes udev

# Buildroot ships python-gobject with -Dpycairo=disabled and no python-pycairo
# dependency, but Onboard needs GI Cairo marshalling: Onboard/KeyGtk.py builds
# cairo.Context objects and hands them to GTK calls such as
# Gdk.cairo_set_source_pixbuf, which only work through that bridge.
#
# This external tree is evaluated after Buildroot's own package files (top-level
# Makefile: package/*/*.mk first, then the BR2_EXTERNAL includes), so appending
# to PYTHON_GOBJECT_DEPENDENCIES here cannot create an ordering edge: the
# stamp's prerequisites were expanded when pkg-generic emitted them. Adding the
# prerequisite to the configure stamp itself does order pycairo first, and
# PYTHON_GOBJECT_CONF_OPTS is read when the configure recipe runs, so that
# append takes effect.
PYTHON_GOBJECT_CONF_OPTS += -Dpycairo=enabled
$(PYTHON_GOBJECT_DIR)/.stamp_configured: python-pycairo

# pygobject's meson configure probes the staging python path for the cairo
# module and its cross pkg-config searches staging only, but upstream
# python-pycairo installs to the target alone: _INSTALL_STAGING is decided when
# the package is evaluated, which is before this file is read, so it cannot be
# turned on from here. The meson infra defines the staging install command
# regardless, so run it from the package's own install step - the prerequisite
# above keeps it ahead of pygobject, and it is what puts py3cairo.pc and the
# cairo module where pygobject looks.
define ONBOARD_STAGE_PYCAIRO
	$(PYTHON_PYCAIRO_INSTALL_STAGING_CMDS)
endef
PYTHON_PYCAIRO_POST_INSTALL_TARGET_HOOKS += ONBOARD_STAGE_PYCAIRO

# Compile the system database on the host: /etc is read-only on the target.
# Overlay copying follows finalize hooks, so read its source directory directly.
define ONBOARD_COMPILE_DEFAULTS
	mkdir -p $(TARGET_DIR)/etc/dconf/db
	$(HOST_DIR)/bin/dconf compile $(TARGET_DIR)/etc/dconf/db/local \
		$(BR2_EXTERNAL_IGRF_PATH)/board/igrf/raspberrypi5/rootfs-overlay/etc/dconf/db/local.d
endef
ONBOARD_TARGET_FINALIZE_HOOKS += ONBOARD_COMPILE_DEFAULTS

# Cross-building native Python extensions and GI typelibs still needs a full build.
$(eval $(python-package))
