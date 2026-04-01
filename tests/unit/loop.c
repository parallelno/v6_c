/* loop.c — loop construct test for v6c */

int main(void)
{
    int sum;
    int i;

    /* while loop: sum 1..10 */
    sum = 0;
    i = 1;
    while (i <= 10) {
        sum = sum + i;
        i = i + 1;
    }
    if (sum != 55) return 1;  /* while sum */

    /* for loop: sum 1..10 again */
    sum = 0;
    for (i = 1; i <= 10; i = i + 1) {
        sum = sum + i;
    }
    if (sum != 55) return 2;  /* for sum */

    return 0;
}
