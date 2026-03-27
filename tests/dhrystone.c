/* dhrystone.c - integer benchmark-style workload for v6c
 *
 * This is a compact Dhrystone-inspired benchmark that stresses:
 * - arithmetic and comparisons
 * - branches and loops
 * - function calls with parameters and return values
 *
 * Result is written to global `bench_result`.
 */

int g1;
int g2;
int g3;
int arr[64];
int bench_result;

int proc_mix(int a, int b, int c)
{
    int x;
    int i;

    x = a + b;
    i = 0;
    while (i < 8) {
        x = x + c;
        x = x ^ (i + a);
        if (x & 1) {
            x = x + b;
        } else {
            x = x - c;
        }
        i = i + 1;
    }
    return x;
}

void proc_arr(int seed)
{
    int i;
    i = 0;
    while (i < 64) {
        arr[i] = seed + i;
        if (i > 0) {
            arr[i] = arr[i] + arr[i - 1];
        }
        i = i + 1;
    }
}

void main(void)
{
    int i;
    int rounds;
    int x;

    g1 = 1;
    g2 = 2;
    g3 = 3;

    proc_arr(7);

    x = 0;
    rounds = 300;
    i = 0;
    while (i < rounds) {
        g1 = proc_mix(g1, g2, g3);
        g2 = proc_mix(g2, g3, g1);
        g3 = proc_mix(g3, g1, g2);

        x = x + (g1 ^ g2) + g3;
        x = x + arr[i & 63];

        if ((x & 3) == 0) {
            x = x + i;
        } else {
            x = x - 1;
        }

        i = i + 1;
    }

    bench_result = x;
}
