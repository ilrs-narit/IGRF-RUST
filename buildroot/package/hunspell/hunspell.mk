################################################################################
#
# hunspell
#
################################################################################

HUNSPELL_VERSION = 1.7.2
HUNSPELL_SITE = $(call github,hunspell,hunspell,v$(HUNSPELL_VERSION))
HUNSPELL_LICENSE = LGPL-2.1+ or GPL-2.0+ or MPL-1.1
HUNSPELL_LICENSE_FILES = COPYING COPYING.LESSER COPYING.MPL
HUNSPELL_INSTALL_STAGING = YES
HUNSPELL_AUTORECONF = YES
HUNSPELL_DEPENDENCIES = host-gettext host-pkgconf
HUNSPELL_CONF_OPTS = --disable-nls --without-readline --without-ui

$(eval $(autotools-package))
