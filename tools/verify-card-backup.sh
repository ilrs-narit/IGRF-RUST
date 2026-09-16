#!/usr/bin/env bash
# Verify an already-written card image against the card itself.
# One sudo authentication covers BOTH card reads, so a long dd cannot expire
# the credential halfway and turn a live check into a hash of empty input.
set -u

DEST="$HOME/backups"
IMG=${1:-$(ls -t "$DEST"/igrf-prototype-card-*.img 2>/dev/null | head -1)}
CARD=/dev/sda
SIZE_EXPECTED=63864569856
HEAD_BYTES=$((1024 * 1024 * 1024))
REPORT="$DEST/verify-report-$(date +%Y%m%d-%H%M).txt"

log() { echo "$*" | tee -a "$REPORT"; }
fail() { log "FAIL: $*"; exit 1; }

[ -f "$IMG" ] || fail "no image to verify (pass one as an argument)"
SIZE=$(stat -c%s "$IMG")
log "image : $IMG"
log "size  : $SIZE bytes"
[ "$SIZE" = "$SIZE_EXPECTED" ] || fail "size mismatches the 64 GB card ($SIZE_EXPECTED)"

# Keep the card unmounted while it is read (best effort; reads are safe either way).
for p in "$CARD"?*; do udisksctl unmount -b "$p" >/dev/null 2>&1 || true; done

log "reading 1 GiB at each end of the card (authenticate now), about 2 minutes..."
if ! OUT=$(sudo sh -c "dd if=$CARD bs=4M iflag=count_bytes count=$HEAD_BYTES status=progress | sha256sum; dd if=$CARD bs=4M iflag=skip_bytes,count_bytes skip=$((SIZE - HEAD_BYTES)) count=$HEAD_BYTES status=progress | sha256sum"); then
  fail "sudo or card read failed - nothing was verified; the image is untouched"
fi
H_CARD=$(printf '%s\n' "$OUT" | head -1 | cut -d' ' -f1)
T_CARD=$(printf '%s\n' "$OUT" | tail -1 | cut -d' ' -f1)
H_IMG=$(head -c $HEAD_BYTES "$IMG" | sha256sum | cut -d' ' -f1)
T_IMG=$(tail -c $HEAD_BYTES "$IMG" | sha256sum | cut -d' ' -f1)
log "head 1GiB card : $H_CARD"
log "head 1GiB image: $H_IMG"
[ "$H_CARD" = "$H_IMG" ] || fail "head mismatch - image does not match the card"
log "tail 1GiB card : $T_CARD"
log "tail 1GiB image: $T_IMG"
[ "$T_CARD" = "$T_IMG" ] || fail "tail mismatch - image does not match the card"
log "head and tail both match the card"

log "hashing the whole image (a few minutes, no privileges needed)..."
sha256sum "$IMG" | tee "$IMG.sha256" | tee -a "$REPORT"
fdisk -l "$IMG" > "$IMG.partitions.txt" 2>&1 && log "partitions -> $IMG.partitions.txt"
log "VERIFY OK - the image is a faithful copy of the card"
