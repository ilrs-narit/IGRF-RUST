#!/bin/sh
set -eu

# Buildroot runs hooks from its source root and supplies BUILD_DIR,
# BINARIES_DIR and BR2_CONFIG. Reuse its empty-root genimage wrapper.
# Fixed inputs/layout; byte-identical filesystem images are not asserted.
# Integration prerequisite for #16: BR2_PACKAGE_HOST_E2FSPROGS=y. genimage
# alone does not provide mke2fs; do not silently use the host OS version.
: "${HOST_DIR:?missing Buildroot host directory}"
if [ ! -x "${HOST_DIR}/sbin/mke2fs" ]; then
    echo 'Enable BR2_PACKAGE_HOST_E2FSPROGS=y before generating data.ext4' >&2
    exit 1
fi
BOARD_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
exec support/scripts/genimage.sh -c "${BOARD_DIR}/genimage.cfg"
