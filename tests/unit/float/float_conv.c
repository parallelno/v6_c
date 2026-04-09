float f;
int i;

int main(void) {
    /* int -> float -> int round-trip */
    i = 42;
    f = (float)i;
    i = (int)f;
    if (i != 42) return 1;

    /* zero */
    i = 0;
    f = (float)i;
    i = (int)f;
    if (i != 0) return 2;

    /* negative */
    i = -100;
    f = (float)i;
    i = (int)f;
    if (i != -100) return 3;

    /* 1 */
    i = 1;
    f = (float)i;
    i = (int)f;
    if (i != 1) return 4;

    /* -1 */
    i = -1;
    f = (float)i;
    i = (int)f;
    if (i != -1) return 5;

    /* truncation: 0.5 -> 0 */
    f = 0.5f;
    i = (int)f;
    if (i != 0) return 6;

    /* truncation: 7.9 -> 7 */
    f = 7.9f;
    i = (int)f;
    if (i != 7) return 7;

    /* truncation: -2.8 -> -2 */
    f = -2.8f;
    i = (int)f;
    if (i != -2) return 8;

    /* implicit int->float promotion */
    f = 10.0f;
    i = 3;
    f = f + (float)i;
    i = (int)f;
    if (i != 13) return 9;

    return 0;
}
