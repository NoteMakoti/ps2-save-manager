# PS2 Save Manager — PS2 homebrew build
#
# Requires the ps2dev toolchain (https://github.com/ps2dev/ps2dev) with
# $PS2SDK set, e.g. the docker image ps2dev/ps2dev:latest:
#
#   docker run --rm -v "$PWD":/src -w /src ps2dev/ps2dev:latest make
#
# Output: PS2SM.ELF (uncompressed) — launch it from uLaunchELF, wLaunchELF,
# OPL's app list, FreeMcBoot, PS2BBL, etc.

EE_BIN = PS2SM.ELF
EE_SRC = src/main.c src/ui.c src/util.c src/mcio.c src/storage.c src/net.c

IRX_MODULES = iomanX fileXio sio2man mcman mcserv padman mmceman \
              usbd bdm bdmfs_fatfs usbmass_bd \
              poweroff ps2dev9 netman smap ps2ip-nm ps2ips smbman

IRX_C = $(foreach m,$(IRX_MODULES),irx/$(subst -,_,$(m))_irx.c)
EE_OBJS = $(EE_SRC:.c=.o) $(IRX_C:.c=.o)

EE_INCS += -Isrc -I$(PS2SDK)/ee/include -I$(PS2SDK)/common/include
EE_CFLAGS += -Wall -Os -std=gnu11 -DNEWLIB_PORT_AWARE -Wno-format-truncation
EE_LIBS = -lps2ips -lnetman -lpatches -lfileXio -lmc -lpad -ldebug -lkernel

all: $(EE_BIN)

irx:
	mkdir -p irx

# bin2c <in> <out.c> <symbol>
irx/%_irx.c: | irx
	$(eval MOD := $(subst _,-,$*))
	$(eval SRC := $(firstword $(wildcard $(PS2SDK)/iop/irx/$(MOD).irx $(PS2SDK)/iop/irx/$*.irx)))
	bin2c $(SRC) $@ $*_irx

clean:
	rm -f $(EE_BIN) $(EE_OBJS)
	rm -rf irx

run: $(EE_BIN)
	ps2client execee host:$(EE_BIN)

include $(PS2SDK)/samples/Makefile.pref
include $(PS2SDK)/samples/Makefile.eeglobal
