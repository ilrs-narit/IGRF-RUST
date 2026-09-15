################################################################################
#
# dconf
#
################################################################################

DCONF_VERSION = 0.40.0
DCONF_SOURCE = dconf-$(DCONF_VERSION).tar.xz
DCONF_SITE = https://download.gnome.org/sources/dconf/0.40
DCONF_LICENSE = LGPL-2.1+
DCONF_LICENSE_FILES = COPYING
DCONF_INSTALL_STAGING = YES
DCONF_DEPENDENCIES = host-pkgconf libglib2 dbus
# Upstream 0.40 uses Meson, not the obsolete autotools build.
DCONF_CONF_OPTS = -Dbash_completion=false -Dman=false -Dgtk_doc=false -Dvapi=false \
	-Dsystemduserunitdir=/usr/lib/systemd/user
HOST_DCONF_DEPENDENCIES = host-pkgconf host-libglib2 host-dbus
# Host installs run without DESTDIR - the host prefix is the install root - so
# the unit directory must sit inside that prefix. Left empty, dconf probes the
# build machine's systemd.pc and installs to its absolute /usr/lib/systemd/user,
# which is outside the prefix and fails with EACCES.
HOST_DCONF_CONF_OPTS = $(DCONF_CONF_OPTS) \
	-Dsystemduserunitdir=$(HOST_DIR)/lib/systemd/user

$(eval $(meson-package))
$(eval $(host-meson-package))
