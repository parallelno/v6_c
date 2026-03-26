; crt0.asm — C runtime startup for Vector 06 (Intel 8080)
; Generated programs are assembled with v6asm (ORG 0x100).
;
; The emitter writes ORG 0x100 and JMP main before including
; generated code, so this file only provides the stack setup
; that precedes the call to main.

; ---------------------------------------------------------------------------
; _start — runtime entry point (jumped to from the emitter header)
; ---------------------------------------------------------------------------
_start:
	LXI SP,0xF800	; set stack below screen memory
	CALL main
	HLT		; halt after main returns
