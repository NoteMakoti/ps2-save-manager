# Builds the PS2 server code (net.c, mcio.c, util.c) for the host, with a
# libmc shim that maps the memory card to ./slot1 — used to test the
# protocol against the desktop client without a console.
CFLAGS = -Wall -g -Iinclude -I../src -DNEWLIB_PORT_AWARE -Wno-format-truncation -Wno-unused-function
SRC = shim.c ../src/net.c ../src/mcio.c ../src/util.c
ps2sm-host: $(SRC) include/*.h
	$(CC) $(CFLAGS) -o $@ $(SRC)
clean:
	rm -f ps2sm-host
