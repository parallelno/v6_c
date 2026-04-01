// opt_small_narrowing.c - integer narrowing optimization
//
// Feature: narrow 32-bit to 16-bit or 16-bit to 8-bit when safe.
// Benefit: smaller instructions and reduced register pressure.

unsigned int x;
unsigned char y;

int main(void) {
    x = 255;
    y = (unsigned char)x;  /* 255 */
    if (x != 255) return 1;
    if (y != 255) return 2;
    return 0;
}
