/* opt_regalloc_pressure.c - register pressure/rematerialization coverage */

int status;

void main(void)
{
    int a;
    int b;
    int c;
    int d;
    int e;
    int f;
    int g;
    int h;
    int pos;
    int neg;
    int safe;

    a = 1;
    b = 2;
    c = 3;
    d = 4;
    e = 5;
    f = 6;
    g = 7;
    h = 8;

    /* positive: many simultaneously live values */
    pos = a + b + c + d;
    pos = pos + e + f + g + h;

    /* negative: dependent chain */
    neg = a;
    neg = neg + 100;
    neg = neg + 100;
    neg = neg + 100;

    /* safety: repeated immediates can rematerialize */
    safe = 42 + 42 + 42 + 42;

    status = pos + neg + safe;
}
