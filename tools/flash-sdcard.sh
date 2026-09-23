#!/usr/bin/env bash
# Write the built Raspberry Pi 4 kiosk image to the card and read it back.
# Run as root:  sudo bash ~/backups/flash-sdcard.sh
# It overwrites /dev/sda only. This build (2026-09-23 09:53) is dev@68d0497:
# the Files tab with copy progress and guarded manual log-segment delete, the
# CSV-failure stop and latched alert (SCRUM-46), and systemd TIME_EPOCH pinned
# to the build time (1790131937, 2026-09-23 02:52 UTC) so an offline boot no
# longer lands in March 2026 (SCRUM-57). The card's existing /data (config,
# SSH/Wi-Fi keys, logs) is overwritten without a backup by operator decision.
set -u

IMG=/home/noobmaster/work/IGRF-RUST/buildroot/output/images/sdcard.img
IMG_SIZE=520093696
IMG_SHA=bcfc8ed1cadd6cd0167e59c073db01c2dbc0505b7225f016ef181d06baea2677
CARD=/dev/sda
CARD_SIZE=63864569856
DEST=/home/noobmaster/backups
MDIR=/home/noobmaster/work/IGRF-RUST/buildroot/output/host/bin/mdir
REPORT="$DEST/flash-pi4-report-$(date +%Y%m%d-%H%M).txt"

log() { echo "$*" | tee -a "$REPORT"; }
fail() {
  log "FAIL: $*"
  exit 1
}

[ "$(id -u)" = 0 ] || fail "run this as root: sudo bash $0"

# ---- the image is the one that was built -----------------------------------
[ -f "$IMG" ] || fail "image not found: $IMG"
[ "$(stat -c%s "$IMG")" = "$IMG_SIZE" ] || fail "image size is not $IMG_SIZE bytes"
log "image : $IMG"
log "checking the image checksum..."
[ "$(sha256sum "$IMG" | cut -d' ' -f1)" = "$IMG_SHA" ] || fail "image sha256 does not match the recorded Pi 4 build"
log "image sha256: ok ($IMG_SHA)"

# ---- the card --------------------------------------------------------------
[ -b "$CARD" ] || fail "$CARD is not a block device"
[ "$(cat /sys/block/${CARD#/dev/}/removable)" = 1 ] || fail "$CARD is not removable - refusing"
[ "$(blockdev --getsize64 "$CARD")" = "$CARD_SIZE" ] || fail "$CARD is not the 64 GB card this project uses"
for p in "$CARD"?*; do udisksctl unmount -b "$p" >/dev/null 2>&1 || true; done
findmnt -rno SOURCE | grep -q "^$CARD" && fail "card is still mounted - unmount it first"
log "card  : $CARD ($CARD_SIZE bytes, removable, unmounted)"

cat <<TXT

Overwriting $CARD with the Raspberry Pi 4 kiosk image.
This overwrites the current card, including its Wi-Fi profiles and SSH keys.
An older prototype backup (not a backup of the current card) is stored at:
  $DEST/igrf-prototype-card-20260915-1519.img
TXT
read -r -p "Type the word FLASH and press Enter to continue: " answer
[ "${answer^^}" = FLASH ] || fail "cancelled - nothing was written"

# ---- record which card this was, then write --------------------------------
log "recording the card's head 1 GiB hash before writing..."
HEAD=$(dd if="$CARD" bs=4M iflag=count_bytes count=1073741824 status=none | sha256sum | cut -d' ' -f1)
log "card head before write: $HEAD"

log "writing the image..."
dd if="$IMG" of="$CARD" bs=4M conv=fsync status=progress
RC=$?
[ "$RC" -eq 0 ] || fail "dd exit=$RC - do not trust this card"
sync
blockdev --flushbufs "$CARD"
log "write done, page cache flushed"

# ---- read back from the medium and compare every byte ----------------------
log "reading all $IMG_SIZE bytes back from the card and comparing..."
if cmp -n "$IMG_SIZE" "$CARD" "$IMG"; then
  log "read-back: identical to the image"
else
  fail "read-back differs from the image - rewrite the card"
fi

# ---- partitions, labels, and the Pi 4 boot files ---------------------------
partprobe "$CARD" 2>/dev/null || true
sleep 1
log "partition table:"
sfdisk -d "$CARD" 2>&1 | tee -a "$REPORT" >/dev/null
sfdisk -d "$CARD" 2>/dev/null | grep -q "0x49475246" && log "disk id: ok (0x49475246)" || log "WARNING: unexpected disk id"
N=$(sfdisk -d "$CARD" 2>/dev/null | grep -c "^$CARD")
[ "$N" = 3 ] && log "partitions: ok (3)" || log "WARNING: $N partitions seen"
blkid "${CARD}1" "${CARD}2" "${CARD}3" 2>/dev/null | tee -a "$REPORT" >/dev/null || log "WARNING: blkid saw nothing yet"
blkid "${CARD}3" 2>/dev/null | grep -q 'LABEL="data"' && log "/data: ext4 labelled data" || log "WARNING: /data label not seen (blkid can lag a fresh write)"
if [ -x "$MDIR" ]; then
  if $MDIR -i "${CARD}1" :: 2>/dev/null | grep -qE "^start4"; then
    log "boot partition on the card: start4.elf present (Pi 4 firmware)"
  else
    log "WARNING: start4.elf not seen on the card's boot partition"
  fi
  $MDIR -i "${CARD}1" :: 2>/dev/null | grep -E "^start4|^fixup4|^config|^cmdline|^IMAGE|^BCM271" | tee -a "$REPORT" >/dev/null
else
  log "WARNING: mdir not found, skipped the boot-partition listing"
fi

log "FLASH OK - the card is ready for the Pi 4; keep coil power disconnected"
