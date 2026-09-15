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

# Buildroot disables GI Cairo marshalling by default; GTK draw callbacks need it.
# python-pycairo is defined before BR2_EXTERNAL, so add the staging install
# through its deferred hook rather than changing an already-evaluated flag.
define ONBOARD_STAGE_PYCAIRO
	$(PYTHON_PYCAIRO_INSTALL_STAGING_CMDS)
endef
PYTHON_PYCAIRO_POST_INSTALL_TARGET_HOOKS += ONBOARD_STAGE_PYCAIRO
PYTHON_GOBJECT_DEPENDENCIES += python-pycairo
PYTHON_GOBJECT_CONF_OPTS += -Dpycairo=enabled

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
