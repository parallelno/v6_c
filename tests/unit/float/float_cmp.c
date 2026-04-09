float a;
float b;

int main(void) {
    /* equal */
    a = 5.0f; b = 5.0f;
    if (!(a == b)) return 1;
    if (a != b)    return 2;

    /* not equal */
    a = 5.0f; b = 3.0f;
    if (a == b)    return 3;
    if (!(a != b)) return 4;

    /* less than */
    a = 2.0f; b = 8.0f;
    if (!(a < b))  return 5;
    if (a > b)     return 6;
    if (!(a <= b)) return 7;
    if (a >= b)    return 8;

    /* greater than */
    a = 9.0f; b = 1.0f;
    if (!(a > b))  return 9;
    if (a < b)     return 10;
    if (!(a >= b)) return 11;
    if (a <= b)    return 12;

    /* negative comparisons */
    a = -3.0f; b = 2.0f;
    if (!(a < b))  return 13;
    if (a >= b)    return 14;

    a = -1.0f; b = -5.0f;
    if (!(a > b))  return 15;
    if (a <= b)    return 16;

    return 0;
}
