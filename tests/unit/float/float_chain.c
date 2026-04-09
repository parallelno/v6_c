float a;
float b;
float c;
float d;
int r;

int main(void) {
    /* chained arithmetic: (a + b) * c */
    a = 2.0f; b = 3.0f; c = 4.0f;
    d = (a + b) * c;
    r = (int)d;
    if (r != 20) return 1;

    /* division + subtraction */
    a = 100.0f; b = 5.0f; c = 3.0f;
    d = a / b - c;
    r = (int)d;
    if (r != 17) return 2;

    /* mixed int/float: int compared against cast */
    a = 6.0f; b = 7.0f;
    r = (int)(a * b);
    if (r != 42) return 3;

    /* multiple casts */
    r = 25;
    a = (float)r;
    b = 5.0f;
    c = a / b;
    r = (int)c;
    if (r != 5) return 4;

    return 0;
}
