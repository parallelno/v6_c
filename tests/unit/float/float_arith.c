float a;
float b;
float c;
int r;

int main(void) {
    /* --- addition --- */
    a = 3.0f; b = 2.0f; c = a + b;
    r = (int)c; if (r != 5) return 1;

    /* positive + negative (same magnitude) = 0 */
    a = 7.0f; b = -7.0f; c = a + b;
    r = (int)c; if (r != 0) return 2;

    /* positive + negative (different magnitude) */
    a = 10.0f; b = -3.0f; c = a + b;
    r = (int)c; if (r != 7) return 3;

    /* --- subtraction --- */
    a = 10.0f; b = 4.0f; c = a - b;
    r = (int)c; if (r != 6) return 4;

    a = 3.0f; b = 8.0f; c = a - b;
    r = (int)c; if (r != -5) return 5;

    /* --- multiplication --- */
    a = 6.0f; b = 7.0f; c = a * b;
    r = (int)c; if (r != 42) return 6;

    a = -4.0f; b = 5.0f; c = a * b;
    r = (int)c; if (r != -20) return 7;

    a = -3.0f; b = -3.0f; c = a * b;
    r = (int)c; if (r != 9) return 8;

    /* --- division --- */
    a = 20.0f; b = 4.0f; c = a / b;
    r = (int)c; if (r != 5) return 9;

    a = -15.0f; b = 3.0f; c = a / b;
    r = (int)c; if (r != -5) return 10;

    return 0;
}
