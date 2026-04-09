/* float_neg.c — negation and sign manipulation */
float a;
float b;
int r;

int main(void) {
    /* negate positive */
    a = 7.0f;
    b = -a;
    r = (int)b;
    if (r != -7) return 1;

    /* negate negative */
    a = -12.0f;
    b = -a;
    r = (int)b;
    if (r != 12) return 2;

    /* double negate = identity */
    a = 25.0f;
    b = -(-a);
    r = (int)b;
    if (r != 25) return 3;

    /* negate then add */
    a = 10.0f;
    b = -a + 3.0f;
    r = (int)b;
    if (r != -7) return 4;

    /* negate in multiplication */
    a = 4.0f;
    b = -a * 3.0f;
    r = (int)b;
    if (r != -12) return 5;

    /* subtract from negated */
    a = 5.0f;
    b = -a - 3.0f;
    r = (int)b;
    if (r != -8) return 6;

    /* negate zero */
    a = 0.0f;
    b = -a;
    r = (int)b;
    if (r != 0) return 7;

    return 0;
}
