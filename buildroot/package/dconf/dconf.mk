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
DCONF_CONF_OPTS = -Dbash_completion=false -Dman=false -Dgtk_doc=false -Dvapi=false
HOST_DCONF_DEPENDENCIES = host-pkgconf host-libglib2 host-dbus
HOST_DCONF_CONF_OPTS = $(DCONF_CONF_OPTS)

$(eval $(meson-package))
$(eval $(host-meson-package))
