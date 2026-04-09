/* float_range.c — larger values and multi-step computations */
float a;
float b;
float c;
int r;

int main(void) {
    /* large multiply: 100 * 100 = 10000 */
    a = 100.0f;
    b = 100.0f;
    c = a * b;
    r = (int)c;
    if (r != 10000) return 1;

    /* large add: 9000 + 999 = 9999 */
    a = 9000.0f;
    b = 999.0f;
    c = a + b;
    r = (int)c;
    if (r != 9999) return 2;

    /* large sub: 20000 - 12345 = 7655 */
    a = 20000.0f;
    b = 12345.0f;
    c = a - b;
    r = (int)c;
    if (r != 7655) return 3;

    /* large div: 10000 / 25 = 400 */
    a = 10000.0f;
    b = 25.0f;
    c = a / b;
    r = (int)c;
    if (r != 400) return 4;

    /* small values: 1 + 1 = 2 */
    a = 1.0f;
    b = 1.0f;
    c = a + b;
    r = (int)c;
    if (r != 2) return 5;

    /* negative large: -500 * 10 = -5000 */
    a = -500.0f;
    b = 10.0f;
    c = a * b;
    r = (int)c;
    if (r != -5000) return 6;

    /* chained ops on large: (300 + 200) * 2 - 100 = 900 */
    a = 300.0f;
    b = 200.0f;
    c = (a + b) * 2.0f - 100.0f;
    r = (int)c;
    if (r != 900) return 7;

    return 0;
}
