    device zxspectrum48 ; There is no ZX Spectrum, it is needed for the sjasmplus assembler.
    org 100h
__begin:
__entry:
__init:
; 26 void __init() {
; 27     /* Zeroing uninitialized variables */
; 28     asm {

        ld   de, __bss
        xor  a
__init_loop:
        ld   (de), a
        inc  de
        ld   hl, 10000h - __end
        add  hl, de
        jp   nc, __init_loop

; 29         ld   de, __bss
; 30         xor  a
; 31 __init_loop:
; 32         ld   (de), a
; 33         inc  de
; 34         ld   hl, 10000h - __end
; 35         add  hl, de
; 36         jp   nc, __init_loop
; 37     }
; 38 
; 39     /* Init stack */
; 40 #if __has_include(<c8080/initstack.inc>) && !defined(ARCH_CPM_CCP) && !defined(ARCH_CPM_BDOS) && !defined(ARCH_CPM_BIOS)
; 41 #include <c8080/initstack.inc>
; 42 #endif
; 43 
; 44 #ifdef ARCH_CPM_CCP /* CCP remains in memory */
; 45     // clang-format off
; 46     asm {
; 47         pop  de
; 48         ld   a, (7)
; 49         sub  8
; 50         ld   h, a
; 51         ld   l, 0
; 52         ld   sp, hl
; 53         push de
; 54     }
; 55     // clang-format on
; 56 #endif
; 57 
; 58 #ifdef ARCH_CPM_BDOS /* BDOS remains in memory */
; 59     asm {
; 60         ld   a, (7)
; 61         ld   h, a
; 62         ld   l, 0
; 63         ld   sp, hl
; 64         ld   hl, 0
; 65         push hl
; 66     }
; 67 #endif
; 68 
; 69 #ifdef ARCH_CPM_BIOS /* BIOS remains in memory */
; 70 #error TODO
; 71 #endif
; 72 
; 73     main(0, NULL);
	ld hl, 0
	ld (__a_1_main), hl
main:
; 23 int main(int argc, char** argv) {
	ld (__a_2_main), hl
; 24     int i;
; 25     int z;
; 26 
; 27     argc = argc;
	ld hl, (__a_1_main)
	ld (__a_1_main), hl
; 28     argv = argv;
	ld hl, (__a_2_main)
	ld (__a_2_main), hl
; 29 
; 30     i = 0;
	ld hl, 0
	ld (main_i), hl
; 31     while (i < 16) {
l_0:
	ld de, 16
	call __o_sub_16
	jp p, l_1
; 32         g_arr[i] = i * 3 + 1;
	ld hl, (main_i)
	ld de, 3
	call __o_mul_i16
	inc hl
	push hl
	ld de, g_arr
	ld hl, (main_i)
	add hl, hl
	add hl, de
	pop de
	ld (hl), e
	inc hl
	ld (hl), d
; 33         i = i + 1;
	ld hl, (main_i)
	inc hl
	ld (main_i), hl
	jp l_0
l_1:
; 34     }
; 35 
; 36     z = mix(11, 7);
	ld hl, 11
	ld (__a_1_mix), hl
	ld hl, 7
	call mix
	ld (main_z), hl
; 37     return z;
	ret
__o_sub_16:
; 265 void __o_sub_16() {
; 266     asm {

        ld   a, l
        sub  e
        ld   l, a
        ld   a, h
        sbc  d
        ld   h, a

	ret
__o_mul_i16:
; 349 void __o_mul_i16() {
; 350     (void)__o_minus_16;
; 351     (void)__o_mul_u16;
; 352     asm {

        ld   a, h
        add  a
        jp   nc, __o_mul_i16_1  ; hl - positive

        call __o_minus_16

        ld   a, d
        add  a
        jp   nc, __o_mul_i16_2  ; hl - negative, de - positive

        ex   hl, de
        call __o_minus_16
        ex   hl, de

        jp   __o_mul_u16 ; hl & de - negative

__o_mul_i16_1:
        ld   a, d
        add  a
        jp   nc, __o_mul_u16  ; hl & de - positive

        ex   hl, de
        call __o_minus_16
        ex   hl, de

__o_mul_i16_2:
        call __o_mul_u16
        jp   __o_minus_16

	ret
mix:
; 5 int mix(int x, int y) {
	ld (__a_2_mix), hl
; 6     int i;
; 7     int s;
; 8 
; 9     s = x + y;
	ld hl, (__a_1_mix)
	ex hl, de
	ld hl, (__a_2_mix)
	add hl, de
	ld (mix_s), hl
; 10     i = 0;
	ld hl, 0
	ld (mix_i), hl
; 11     while (i < 16) {
l_2:
	ld de, 16
	call __o_sub_16
	jp p, l_3
; 12         s = s + ((g_arr[i] + i) & 255);
	ld de, g_arr
	ld hl, (mix_i)
	add hl, hl
	add hl, de
	ld e, (hl)
	inc hl
	ld d, (hl)
	ld hl, (mix_i)
	add hl, de
	ld de, 255
	call __o_and_16
	ex hl, de
	ld hl, (mix_s)
	add hl, de
	ld (mix_s), hl
; 13         if ((s & 1) == 0) {
	ld de, 1
	call __o_and_16
	ld a, h
	or l
	jp nz, l_4
; 14             s = s + g_a;
	ld hl, (mix_s)
	ex hl, de
	ld hl, (g_a)
	add hl, de
	ld (mix_s), hl
	jp l_5
l_4:
; 15         } else {
; 16             s = s - g_b;
	ld hl, (g_b)
	ex hl, de
	ld hl, (mix_s)
	call __o_sub_16
	ld (mix_s), hl
l_5:
; 17         }
; 18         i = i + 1;
	ld hl, (mix_i)
	inc hl
	ld (mix_i), hl
	jp l_2
l_3:
; 19     }
; 20     return s;
	ld hl, (mix_s)
	ret
__o_minus_16:
; 235 void __o_minus_16() {
; 236     asm {

        xor  a
        sub  l
        ld   l, a
        ld   a, 0
        sbc  h
        ld  h, a

	ret
__o_mul_u16:
; 326 void __o_mul_u16() {
; 327     asm {

        ld   b, h
        ld   c, l
        ld   hl, 0
        ld   a, 17
__o_mul_u16_l1:
        dec  a
        ret  z
        add  hl, hl
        ex   hl, de
        add  hl, hl
        ex   hl, de
        jp   nc, __o_mul_u16_l1
        add  hl, bc
        jp   __o_mul_u16_l1

	ret
__o_and_16:
; 280 void __o_and_16() {
; 281     asm {

        ld   a, h
        and  d
        ld   h, a
        ld   a, l
        and  e
        ld   l, a

	ret
g_a:
	dw 3
g_b:
	dw 5
__bss:
g_arr:
	ds 32
__static_stack:
	ds 16
__end:
__s___init equ __static_stack + 16
__s_main equ __static_stack + 8
__a_1_main equ __s_main + 4
__a_2_main equ __s_main + 6
main_i equ __s_main + 0
main_z equ __s_main + 2
__s_mix equ __static_stack + 0
__a_1_mix equ __s_mix + 4
__a_2_mix equ __s_mix + 6
__s___o_sub_16 equ __static_stack + 0
__s___o_mul_i16 equ __static_stack + 0
mix_s equ __s_mix + 2
mix_i equ __s_mix + 0
__s___o_minus_16 equ __static_stack + 0
__s___o_mul_u16 equ __static_stack + 0
__s___o_and_16 equ __static_stack + 0
    savebin "dual_c8080.bin", __begin, __bss - __begin
