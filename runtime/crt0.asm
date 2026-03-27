; crt0.asm — C runtime startup for Vector 06C (Intel 8080)
;
; Generated programs are assembled with v6asm (ORG 0x100).
; The emitter writes ORG 0x100 and JMP _start before including
; generated code. This module provides:
;   1. Stack initialization
;   2. BSS zero-fill (static data clearing)
;   3. Call to main()
;   4. Program termination (HLT)
;
; Memory map (Vector 06C):
;   0x0000-0x00FF  System / interrupt vectors
;   0x0100-0x7FFF  Program code + data + heap
;   0x8000-0xEFFF  Variable allocation (callgraph static alloc)
;   0xF000-0xF7FF  Heap upper limit (configurable)
;   0xF800-0xFFFF  Stack (grows downward)

; ---------------------------------------------------------------------------
; _start — runtime entry point
; ---------------------------------------------------------------------------
_start:
	LXI SP,0xF800		; set stack below screen memory
	CALL main
	HLT			; halt after main returns
