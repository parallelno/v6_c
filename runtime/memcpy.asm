; memcpy.asm — memory operations for Intel 8080
;
; C calling convention: arg0 in HL, arg1 in DE, arg2+ on stack.
; Return value in HL (16-bit).
; Clobbers: A, B, C, D, E, H, L, flags

; ---------------------------------------------------------------------------
; memcpy — copy n bytes from src to dest
;   C: void *memcpy(void *dest, void *src, unsigned int n)
;   Entry: HL = dest, DE = src, n at [SP+2]
;   Exit:  HL = dest
; ---------------------------------------------------------------------------
memcpy:
	; Read n from stack
	PUSH H			; save dest
	LXI H,4		; 2(push) + 2(ret_addr)
	DAD SP
	MOV C,M
	INX H
	MOV B,M			; BC = n
	POP H			; HL = dest

	MOV A,B
	ORA C
	RZ			; count == 0 → done
	PUSH H			; save dest for return
	; Use DCR C / DCR B loop (faster than DCX B / ORA)
	INR B			; pre-increment B for outer loop
	XRA A
	ORA C
	JZ __memcpy_outer
__memcpy_loop:
	LDAX D			; A = *(src)
	MOV M,A			; *(dest) = A
	INX H
	INX D
	DCR C
	JNZ __memcpy_loop
__memcpy_outer:
	DCR B
	JNZ __memcpy_loop
	POP H			; return original dest
	RET

; ---------------------------------------------------------------------------
; memset — fill n bytes at dest with value c
;   C: void *memset(void *dest, int c, unsigned int n)
;   Entry: HL = dest, DE = c (low byte E), n at [SP+2]
;   Exit:  HL = dest
; ---------------------------------------------------------------------------
memset:
	; Read n from stack
	PUSH H			; save dest
	LXI H,4		; 2(push) + 2(ret_addr)
	DAD SP
	MOV C,M
	INX H
	MOV B,M			; BC = n
	POP H			; HL = dest

	MOV A,B
	ORA C
	RZ			; n == 0 → done
	PUSH H			; save dest for return
	MOV A,E			; A = fill value (low byte of c)
	INR B			; pre-increment B for outer loop
	PUSH PSW		; save fill value + check if C==0
	XRA A
	ORA C
	POP PSW
	JZ __memset_outer
__memset_loop:
	MOV M,A
	INX H
	DCR C
	JNZ __memset_loop
__memset_outer:
	DCR B
	JNZ __memset_loop
	POP H			; return original dest
	RET

; ---------------------------------------------------------------------------
; strlen — return length of null-terminated string
;   C: unsigned int strlen(char *s)
;   Entry: HL = s
;   Exit:  HL = length (16-bit)
; ---------------------------------------------------------------------------
strlen:
	LXI B,0		; BC = count = 0
__strlen_loop:
	MOV A,M
	ORA A
	JZ __strlen_done
	INX H
	INX B
	JMP __strlen_loop
__strlen_done:
	MOV H,B
	MOV L,C
	RET

; ---------------------------------------------------------------------------
; strcmp — compare two null-terminated strings
;   C: int strcmp(char *s1, char *s2)
;   Entry: HL = s1, DE = s2
;   Exit:  HL = <0 if s1<s2, 0 if equal, >0 if s1>s2
; ---------------------------------------------------------------------------
strcmp:
__strcmp_loop:
	LDAX D			; A = *s2
	MOV B,A			; B = *s2
	MOV A,M			; A = *s1
	CMP B			; compare *s1 with *s2
	JNZ __strcmp_diff
	ORA A			; check for null terminator
	JZ __strcmp_equal
	INX H
	INX D
	JMP __strcmp_loop
__strcmp_diff:
	; A = *s1, B = *s2. Return *s1 - *s2 sign-extended to 16 bits.
	SUB B			; A = *s1 - *s2
	MOV L,A
	MVI H,0
	ORA A
	RP			; positive → done
	MVI H,0xFF		; negative → sign-extend
	RET
__strcmp_equal:
	LXI H,0
	RET
