/* float_pow2.c — powers of two and exact representation tests */
float a;
float b;
float c;
int r;

int main(void) {
    /* 2 * 2 = 4 */
    a = 2.0f;
    b = a * a;
    r = (int)b;
    if (r != 4) return 1;

    /* 4 * 4 = 16 */
    a = 4.0f;
    b = a * a;
    r = (int)b;
    if (r != 16) return 2;

    /* 8 * 8 = 64 */
    a = 8.0f;
    b = a * a;
    r = (int)b;
    if (r != 64) return 3;

    /* 16 * 16 = 256 */
    a = 16.0f;
    b = a * a;
    r = (int)b;
    if (r != 256) return 4;

    /* 256 / 16 = 16 */
    a = 256.0f;
    b = 16.0f;
    c = a / b;
    r = (int)c;
    if (r != 16) return 5;

    /* 1024 / 32 = 32 */
    a = 1024.0f;
    b = 32.0f;
    c = a / b;
    r = (int)c;
    if (r != 32) return 6;

    /* 128 + 128 = 256 */
    a = 128.0f;
    b = 128.0f;
    c = a + b;
    r = (int)c;
    if (r != 256) return 7;

    /* 512 - 256 = 256 */
    a = 512.0f;
    b = 256.0f;
    c = a - b;
    r = (int)c;
    if (r != 256) return 8;

    return 0;
}
