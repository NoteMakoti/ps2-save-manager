/*
 * PS2 Save Manager — PS2 homebrew side.
 * Shared definitions.
 */
#ifndef PS2SM_COMMON_H
#define PS2SM_COMMON_H

#include <tamtypes.h>
#include <stddef.h>

#define PS2SM_VERSION "0.1.0"
#define PS2SM_DIR "PS2SM"

/* iomanX / mcman open flags (SCE values, NOT newlib's O_*). */
#define IOX_RDONLY 0x0001
#define IOX_WRONLY 0x0002
#define IOX_RDWR 0x0003
#define IOX_CREAT 0x0200
#define IOX_TRUNC 0x0400

/* Directory entry mode bits (same as on the card). */
#define DF_READ 0x0001
#define DF_WRITE 0x0002
#define DF_EXECUTE 0x0004
#define DF_PROTECTED 0x0008
#define DF_FILE 0x0010
#define DF_DIR 0x0020
#define DF_CLOSED 0x0080
#define DF_0400 0x0400
#define DF_PSX 0x1000
#define DF_HIDDEN 0x2000
#define DF_EXISTS 0x8000
#define DF_SAVE_DIR 0x8427
#define DF_SAVE_FILE 0x8497

/* Protocol status codes (docs/PROTOCOL.md). */
#define ST_OK 0
#define ST_FAIL -1
#define ST_NOT_FOUND -2
#define ST_NO_CARD -3
#define ST_NO_SPACE -4
#define ST_EXISTS -5
#define ST_BAD_REQUEST -6
#define ST_UNSUPPORTED -7
#define ST_IO -8

/* Growable byte buffer. */
typedef struct
{
    u8 *data;
    size_t len;
    size_t cap;
} buf_t;

void buf_init(buf_t *b);
void buf_free(buf_t *b);
int buf_reserve(buf_t *b, size_t extra);
int buf_put(buf_t *b, const void *p, size_t n);
int buf_u8(buf_t *b, u8 v);
int buf_u16(buf_t *b, u16 v);
int buf_u32(buf_t *b, u32 v);
int buf_str(buf_t *b, const char *s);
int buf_bstr(buf_t *b, const void *p, u16 n);
int buf_zero(buf_t *b, size_t n);

/* Runtime configuration (PS2SM.CNF). */
typedef struct
{
    int use_dhcp;
    char ip[16], netmask[16], gateway[16];
    int port;
    char smb_ip[16];
    int smb_port;
    char smb_share[64];
    char smb_user[64];
    char smb_pass[64];
    char smb_dir[128];
} config_t;

extern config_t g_cfg;
extern int g_have_net, g_have_usb, g_have_smb, g_have_mmce;
extern char g_ip_str[32];
extern char g_boot_dir[256];

void config_defaults(config_t *c);
int config_load(config_t *c);

/* Simple Shift-JIS -> ASCII for the debug font. */
void sjis_to_ascii(const u8 *in, int inlen, char *out, int outlen);

#endif
