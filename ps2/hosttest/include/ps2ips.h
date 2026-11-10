/* host shim: real BSD sockets */
#pragma once
#include <sys/socket.h>
#include <tamtypes.h>
#include <netinet/in.h>
typedef struct { char netif_name[4]; struct in_addr ipaddr, netmask, gw; u32 dhcp_enabled, dhcp_status; u8 hw_addr[8]; } t_ip_info;
static inline int ps2ip_init(void) { return 0; }
