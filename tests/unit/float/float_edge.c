float a;
float b;
float c;
int r;

int main(void) {
    /* 0 + x = x */
    a = 0.0f; b = 5.0f; c = a + b;
    r = (int)c; if (r != 5) return 1;

    /* x + 0 = x */
    a = 5.0f; b = 0.0f; c = a + b;
    r = (int)c; if (r != 5) return 2;

    /* 0 * x = 0 */
    a = 0.0f; b = 99.0f; c = a * b;
    r = (int)c; if (r != 0) return 3;

    /* x * 0 = 0 */
    a = 99.0f; b = 0.0f; c = a * b;
    r = (int)c; if (r != 0) return 4;

    /* 0 / x = 0 */
    a = 0.0f; b = 5.0f; c = a / b;
    r = (int)c; if (r != 0) return 5;

    /* x - x = 0 */
    a = 123.0f; b = 123.0f; c = a - b;
    r = (int)c; if (r != 0) return 6;

    /* multiply by 1 */
    a = 42.0f; b = 1.0f; c = a * b;
    r = (int)c; if (r != 42) return 8;

    /* large exponent difference - small operand contributes nothing */
    a = 10000.0f; b = 0.001f; c = a + b;
    r = (int)c; if (r != 10000) return 9;

    return 0;
}
