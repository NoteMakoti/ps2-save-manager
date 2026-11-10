#ifndef PS2SM_STORAGE_H
#define PS2SM_STORAGE_H

#include "common.h"

typedef enum { TARGET_USB = 0, TARGET_SMB = 1 } target_t;

/* Root folder for backups on a target, e.g. "mass:/PS2SM" */
void storage_root(target_t t, char *out, int n);
int storage_available(target_t t);
const char *storage_name(target_t t);

/* Mount the SMB share from the config. Returns 0 on success. */
int smb_connect(char *err, int errlen);

/* Lists *.psu in the target root. names[] gets malloc'ed strings. */
int storage_list_psu(target_t t, char ***names, int *count);
void storage_free_list(char **names, int count);

int storage_write(target_t t, const char *name, const u8 *data, int len);
int storage_read(target_t t, const char *name, u8 **data, int *len);

/* MMCE (MemCard PRO2, SD2PSX...) via mmceman devctl. port 0/1 */
int mmce_present(int port);
int mmce_get(int port, int *card, int *channel, char *gameid, int gidlen);
/* what: 0 card, 1 channel; mode: 0 set, 1 next, 2 prev */
int mmce_set(int port, int what, int mode, int num);

#endif
