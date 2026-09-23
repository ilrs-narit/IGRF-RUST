include $(sort $(wildcard $(BR2_EXTERNAL_IGRF_PATH)/package/*/*.mk))

# SCRUM-57: pin systemd's compiled TIME_EPOCH to the image build time. With no
# SOURCE_DATE_EPOCH exported and no .git in the systemd tarball, systemd's
# meson falls back to the mtime of its NEWS file — the upstream 258.7 release
# date 2026-03-13 — which stamped the RTC-less Pi's clock.
SYSTEMD_CONF_OPTS += -Dtime-epoch=$(shell date +%s)
