/* float_cond.c — float in conditional (if/else) and ternary-like patterns */
float a;
float b;
float c;
int r;

int main(void) {
    /* if-else selecting larger */
    a = 3.0f;
    b = 9.0f;
    if (a > b) { c = a; } else { c = b; }
    r = (int)c;
    if (r != 9) return 1;

    /* if-else selecting smaller */
    a = 12.0f;
    b = 4.0f;
    if (a < b) { c = a; } else { c = b; }
    r = (int)c;
    if (r != 4) return 2;

    /* negative comparison drives branch */
    a = -10.0f;
    b = -2.0f;
    if (a < b) { c = a; } else { c = b; }
    r = (int)c;
    if (r != -10) return 3;

    /* equality branch */
    a = 7.0f;
    b = 7.0f;
    r = 99;
    if (a == b) { r = 0; }
    if (r != 0) return 4;

    /* inequality branch */
    a = 7.0f;
    b = 8.0f;
    r = 0;
    if (a != b) { r = 1; }
    if (r != 1) return 5;

    /* chained comparisons via nested if */
    a = 5.0f;
    b = 10.0f;
    c = 3.0f;
    r = 0;
    if (a > c) {
        if (a < b) {
            r = 1;   /* a between c and b */
        }
    }
    if (r != 1) return 6;

    return 0;
}
