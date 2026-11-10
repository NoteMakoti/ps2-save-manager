#ifndef PS2SM_NET_H
#define PS2SM_NET_H

/* Bring up the IOP TCP/IP stack (DHCP or static from config).
 * Returns 0 when an address is configured. */
int net_init(void (*progress)(const char *msg));

/* Server: call net_server_start once, then net_server_poll() from the UI
 * loop; it services at most one request per call and returns quickly. */
int net_server_start(int port);
void net_server_poll(int timeout_ms);
void net_server_stop(void);

/* Status for the UI */
extern char g_net_status[96];
extern char g_net_last[96];
extern int g_net_requests;

#endif
