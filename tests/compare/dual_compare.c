int g_a = 3;
int g_b = 5;
int g_arr[16];

int mix(int x, int y) {
    int i;
    int s;

    s = x + y;
    i = 0;
    while (i < 16) {
        s = s + ((g_arr[i] + i) & 255);
        if ((s & 1) == 0) {
            s = s + g_a;
        } else {
            s = s - g_b;
        }
        i = i + 1;
    }
    return s;
}

int main(int argc, char** argv) {
    int i;
    int z;

    argc = argc;
    argv = argv;

    i = 0;
    while (i < 16) {
        g_arr[i] = i * 3 + 1;
        i = i + 1;
    }

    z = mix(11, 7);
    return z;
}
