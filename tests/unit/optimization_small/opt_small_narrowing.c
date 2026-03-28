// opt_small_narrowing.c - integer narrowing optimization
//
// Feature: narrow 32-bit to 16-bit or 16-bit to 8-bit when safe.
// Benefit: smaller instructions and reduced register pressure.
// Example:
//   uint16_t x = 255; // can remain 8-bit if usage allows

unsigned int x;
unsigned char y;

void main(void) {
    x = 255;
    y = (unsigned char)x;
}
