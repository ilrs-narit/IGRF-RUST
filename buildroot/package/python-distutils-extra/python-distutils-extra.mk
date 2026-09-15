################################################################################
#
# python-distutils-extra
#
################################################################################

PYTHON_DISTUTILS_EXTRA_VERSION = 3.1
PYTHON_DISTUTILS_EXTRA_SOURCE = python-distutils-extra_$(PYTHON_DISTUTILS_EXTRA_VERSION).tar.xz
PYTHON_DISTUTILS_EXTRA_SITE = https://deb.debian.org/debian/pool/main/p/python-distutils-extra
PYTHON_DISTUTILS_EXTRA_SETUP_TYPE = setuptools
PYTHON_DISTUTILS_EXTRA_LICENSE = GPL-2.0+
PYTHON_DISTUTILS_EXTRA_LICENSE_FILES = LICENSE
HOST_PYTHON_DISTUTILS_EXTRA_DEPENDENCIES = host-intltool host-gettext

$(eval $(python-package))
$(eval $(host-python-package))
