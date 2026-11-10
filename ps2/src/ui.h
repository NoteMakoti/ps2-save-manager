#ifndef PS2SM_UI_H
#define PS2SM_UI_H

#include <libpad.h>

/* Buttons, already edge-detected ("pressed this frame"), with auto-repeat
 * for the d-pad. */
#define BTN_UP PAD_UP
#define BTN_DOWN PAD_DOWN
#define BTN_LEFT PAD_LEFT
#define BTN_RIGHT PAD_RIGHT
#define BTN_CROSS PAD_CROSS
#define BTN_CIRCLE PAD_CIRCLE
#define BTN_TRIANGLE PAD_TRIANGLE
#define BTN_SQUARE PAD_SQUARE
#define BTN_START PAD_START
#define BTN_SELECT PAD_SELECT
#define BTN_L1 PAD_L1
#define BTN_R1 PAD_R1
#define BTN_L2 PAD_L2
#define BTN_R2 PAD_R2

#define UI_ROWS 22

void ui_init(void);
int ui_pad_init(void);
/* Wait for the next frame and return newly pressed buttons. */
u32 ui_buttons(void);

void ui_header(const char *title);
void ui_footer(const char *hint);
void ui_line(int row, int highlight, const char *fmt, ...) __attribute__((format(printf, 3, 4)));
void ui_message(const char *title, const char *fmt, ...) __attribute__((format(printf, 2, 3)));
int ui_confirm(const char *fmt, ...) __attribute__((format(printf, 1, 2)));
void ui_status(const char *fmt, ...) __attribute__((format(printf, 1, 2)));

/* A scrolling list menu. Returns the selected index or -1 (circle).
 * `extra` receives the button that confirmed the choice. */
typedef void (*ui_item_fn)(int index, char *out, int outlen, void *ctx);
int ui_list(const char *title, const char *hint, int count, ui_item_fn item, void *ctx, int *cursor, u32 accept, u32 *pressed);

#endif
