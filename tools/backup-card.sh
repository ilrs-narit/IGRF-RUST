#!/usr/bin/env bash
# Read-only image backup of the prototype microSD before it is reflashed.
# This script only READS /dev/sda and only WRITES inside $HOME/backups.
set -u

CARD=/dev/sda
DEST="$HOME/backups"
STAMP=$(date +%Y%m%d-%H%M)
IMG="$DEST/igrf-prototype-card-$STAMP.img"
REPORT="$DEST/backup-report-$STAMP.txt"

mkdir -p "$DEST"
log() { echo "$*" | tee -a "$REPORT"; }
fail() { log "FAIL: $*"; exit 1; }

# --- guards: fail loudly rather than archive the wrong disk -----------------
[ -b "$CARD" ] || fail "$CARD is not a block device"
SIZE=$(sudo blockdev --getsize64 "$CARD") || fail "cannot read $CARD (sudo?)"
[ "$SIZE" = 63864569856 ] || fail "unexpected size $SIZE bytes; expected the 64 GB card (63864569856)"
[ "$(cat /sys/block/${CARD#/dev/}/removable)" = 1 ] || fail "$CARD is not removable - refusing"
log "card   : $CARD  $SIZE bytes  (removable, MXT-USB serial 150101v01)"
log "image  : $IMG"
log "report : $REPORT"

# --- the card must not be mounted while it is imaged ------------------------
if findmnt -rno SOURCE | grep -q "^$CARD"; then
  log "unmounting card partitions via udisksctl..."
  for p in "$CARD"?*; do udisksctl unmount -b "$p" >/dev/null 2>&1 || true; done
  sleep 1
  findmnt -rno SOURCE | grep -q "^$CARD" && fail "still mounted - unmount it first"
fi
log "mounted: no"

# --- the backup itself (progress on the terminal, not in the report) --------
log "reading the card now; expect roughly 30-60 minutes at USB2 speed"
sudo dd if="$CARD" of="$IMG" bs=4M conv=fsync status=progress
RC=$?
[ "$RC" -eq 0 ] || fail "dd exit=$RC - the image is NOT trustworthy, do not flash anything"
sync
log "dd exit: 0"

FSIZE=$(stat -c%s "$IMG")
[ "$FSIZE" = "$SIZE" ] || fail "size mismatch: image=$FSIZE card=$SIZE"
log "size   : ok ($FSIZE bytes = card size)"

# --- spot-check 1 GiB at each end, both card reads inside ONE sudo session --
log "reading 1 GiB at each end of the card to compare (authenticate now)..."
HEAD_BYTES=$((1024 * 1024 * 1024))
if ! OUT=$(sudo sh -c "dd if=$CARD bs=4M iflag=count_bytes count=$HEAD_BYTES status=none | sha256sum; dd if=$CARD bs=4M iflag=skip_bytes,count_bytes skip=$((SIZE - HEAD_BYTES)) count=$HEAD_BYTES status=none | sha256sum"); then
  fail "sudo or card read failed during verification - the image file is written but unverified; re-run verify-card-backup.sh (no need to read the card again)"
fi
H_CARD=$(printf '%s\n' "$OUT" | head -1 | cut -d' ' -f1)
T_CARD=$(printf '%s\n' "$OUT" | tail -1 | cut -d' ' -f1)
H_IMG=$(head -c $HEAD_BYTES "$IMG" | sha256sum | cut -d' ' -f1)
T_IMG=$(tail -c $HEAD_BYTES "$IMG" | sha256sum | cut -d' ' -f1)
[ "$H_CARD" = "$H_IMG" ] || fail "head mismatch: card=$H_CARD image=$H_IMG"
log "head 1GiB sha256: $H_IMG  (matches card)"
[ "$T_CARD" = "$T_IMG" ] || fail "tail mismatch: card=$T_CARD image=$T_IMG"
log "tail 1GiB sha256: $T_IMG  (matches card)"

# --- archive record ---------------------------------------------------------
sha256sum "$IMG" | tee "$IMG.sha256" | tee -a "$REPORT"
fdisk -l "$IMG" > "$IMG.partitions.txt" 2>&1 && log "partitions -> $IMG.partitions.txt"
sudo tune2fs -l "${CARD}2" 2>/dev/null | grep -E "Filesystem UUID|Filesystem state|Last mount time" | tee -a "$REPORT"
log "restore command: sudo dd if=$IMG of=/dev/sdX bs=4M conv=fsync status=progress"
log "BACKUP OK - the card was only read, never written"
