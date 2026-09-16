################################################################################
#
# iso-codes
#
################################################################################

ISO_CODES_VERSION = 4.18.0
ISO_CODES_SOURCE = iso-codes_$(ISO_CODES_VERSION).orig.tar.xz
ISO_CODES_SITE = https://deb.debian.org/debian/pool/main/i/iso-codes
ISO_CODES_LICENSE = LGPL-2.1+
ISO_CODES_LICENSE_FILES = COPYING
ISO_CODES_DEPENDENCIES = host-python3

define ISO_CODES_BUILD_CMDS
	$(HOST_DIR)/bin/python3 $(@D)/bin/xml_from_json.py iso_639-2 $(@D)/data $(@D)/iso_639.xml
	$(HOST_DIR)/bin/python3 $(@D)/bin/xml_from_json.py iso_3166-1 $(@D)/data $(@D)/iso_3166.xml
endef

# Onboard reads the legacy XML language tables. No executable is needed.
define ISO_CODES_INSTALL_TARGET_CMDS
	mkdir -p $(TARGET_DIR)/usr/share/xml/iso-codes $(TARGET_DIR)/usr/share/iso-codes/json
	$(INSTALL) -m 0644 $(@D)/iso_*.xml $(TARGET_DIR)/usr/share/xml/iso-codes/
	$(INSTALL) -m 0644 $(@D)/data/*.json $(TARGET_DIR)/usr/share/iso-codes/json/
endef

$(eval $(generic-package))
