/* float_loop.c — float used in loop accumulation and counting */
float sum;
float val;
int i;
int r;

int main(void) {
    /* sum 1+2+3+4+5 = 15 */
    sum = 0.0f;
    i = 1;
    while (i <= 5) {
        val = (float)i;
        sum = sum + val;
        i = i + 1;
    }
    r = (int)sum;
    if (r != 15) return 1;

    /* multiply: 1*2*3*4 = 24 */
    sum = 1.0f;
    i = 2;
    while (i <= 4) {
        val = (float)i;
        sum = sum * val;
        i = i + 1;
    }
    r = (int)sum;
    if (r != 24) return 2;

    /* count down: 100 - 10*5 = 50 */
    sum = 100.0f;
    i = 0;
    while (i < 5) {
        sum = sum - 10.0f;
        i = i + 1;
    }
    r = (int)sum;
    if (r != 50) return 3;

    /* divide repeatedly: 64 / 2 / 2 / 2 = 8 */
    sum = 64.0f;
    i = 0;
    while (i < 3) {
        sum = sum / 2.0f;
        i = i + 1;
    }
    r = (int)sum;
    if (r != 8) return 4;

    return 0;
}
