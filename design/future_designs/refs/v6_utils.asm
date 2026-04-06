@memusage_v6_utils:
.include "asm/v6/v6_rnd.asm"
.include "asm/v6/v6_dzx0.asm"

; shared chunk of code to restore SP
; and dismount the RAM Disk
restore_sp:
			lxi sp, TEMP_ADDR
			RAM_DISK_OFF()
			ret


; erase a memory buffer (ram)
; 	hl - source
;	bc - source + len
; out:
;	hl - source + len
;	bc - source + len
; prep: 8cc
; loop: 32-64cc
; copy 4 bytes: 48 + 32*4 + 32 * 1 = 208
; copy 32 bytes: 48 + 32*32 + 32 * 1 = 1104
mem_erase:
			mvi e, 0
; e - filler
mem_fill:
			; hl - source
			; bc - source + len
@loop2:
			mov a, c
@loop:
			mov m, e
			inx h

			cmp l
			jnz @loop
			mov a, b
			cmp h
			jnz @loop2
			ret


; clears a memory buffer using stack operations
; can be used to clear the RAM Disk as well
;		IT CORRUPTS TWO BYTES BEFORE THE BUFFER if disable_int = false!
.macro MEM_ERASE_SP(buf_start, buff_len, command = RAM_DISK_OFF_CMD, disable_int = false)
		.if disable_int
			di
		.endif
			lxi b, >((buf_start + buff_len)<<8) | <(buf_start + buff_len)
			lxi d, buff_len / 32 - 1
			mvi a, command
			call mem_erase_sp
		.if disable_int
			ei
		.endif
.endmacro
; input:
; bc - source + len
; de - length // 32 - 1
; a - RAM Disk activation command
; 		a = 0 to clear the main memory
; use:
; hl
mem_erase_sp:
			RAM_DISK_ON_BANK()
			lxi h, 0x0000
			dad sp
			shld restore_sp + 1
			mov h, b
			mov l, c
mem_erase_sp_filler:
			lxi b, $0000
			sphl
			mvi a, 0xFF
@loop:
			PUSH_B(16)

			dcx d
			cmp d
			jnz @loop
			jmp restore_sp

; fill a memory buffer with a word using stack operations
; can be used to clear RAM Disk memory as well
; input:
; hl - a filler word
; bc - the last addr of a erased buffer + 1
; de - length/32 - 1
; a - RAM Disk activation command
; 		a = 0 to clear the main memory
mem_fill_sp:
			shld mem_erase_sp_filler + 1
			call mem_erase_sp
			lxi h, 0
			shld mem_erase_sp_filler + 1
			ret

; copy a memory buffer (ram to ram )
; in:
; 	hl - source
; 	de - destination
;	bc - len
; prep: 28cc
; loop: 52-72cc
; copy 32 bytes: 28 + 1664 + 20 + 12 = 1712 cc
; copy 128 bytes: 28 + 52*128 + 20 + 12 = 6716 cc
; copy 256 bytes: 28 + 20 + 52*256 + 20*1 + 12 = 13,392 cc
; copy 1024*24 bytes: 28 + 20 + 52*1024*24 + 20*4*24 = 1,279,920 cc
mem_copy:
			; hl - source
			; de - destination
			; bc - len
			; for correct ending the outer loop with dcr b
			inr b
			; enter the outer loop if C=0
			xra a
			ora c
			jz @outer_loop
@loop:
			; copy a byte
			mov a, m
			stax d
			inx h
			inx d
			; check the end
			dcr c
			jnz @loop
@outer_loop:
			dcr b
			jnz @loop
			ret

;========================================
; copy a memory buffer (ram to RAM Disk)
; !!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!
; !!! IT CORRUPTS TWO BYTES BEFORE THE BUFFER !!!
; !!!      IF INTERRUPTIONS ARE ENABLED       !!!
; !!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!
MEM_COPY_WORD_LEN = 5
; in:
; de - source
; hl - destination + len
; bc - len. len must be non-zero and divisible by 2
; a - RAM Disk activation command
; out:
; de - source
; hl - source

; prep: (53 + 10+7) * 4 = 280cc
; loop 32 bytes: 784 + 32= 816 cc
; copy 32 bytes: 280 + 784*1 + 32 = 1096 cc
; copy 128 bytes: 280 + 784*4 + 32 = 3448 cc
; copy 256 bytes: 280 + 784*8 + 32 = 6584 cc
; copy 1024*24 bytes: 280 + 784*768 + 32*96 = 608,536 cc
mem_copy_to_ram_disk:
			shld @dest + 1
			sta @mapping + 1

			; store sp
			lxi h, 0x0000
			dad sp
			shld restore_sp + 1

			; bc - len
			; get the jump addr to start the loop
			mvi a, 0b00011110 ; get the reminder of 32
			ana c
			HL_TO_A_PLUS_INT16(jmp_tbl)
			mov a, m
			inx h
			mov h, m
			mov l, a

			shld @start_loop + 1

			mov h, b
			mov l, c
			dad d
@mapping:
			mvi a, TEMP_BYTE
			RAM_DISK_ON_BANK()

			; TODO: think of how to optimize these two MOVs
			mov b, d
			mov c, e

			mov a, c
@dest:
			lxi sp, TEMP_ADDR
@start_loop:
			jmp TEMP_ADDR

			; hl - source + len
			; bc - source
			; sp - destination + len
mem_copy_to_ram_disk_loop2:
			mov a, c
mem_copy_to_ram_disk_loop:
		.loop 16
			MEM_COPY_WORD()
		.endloop
			; check the end
			cmp l
			jnz mem_copy_to_ram_disk_loop
			mov a, b
			cmp h
			jnz mem_copy_to_ram_disk_loop2
			jmp restore_sp
jmp_tbl:
			.word mem_copy_to_ram_disk_loop + MEM_COPY_WORD_LEN * 0
			.word mem_copy_to_ram_disk_loop + MEM_COPY_WORD_LEN * 15
			.word mem_copy_to_ram_disk_loop + MEM_COPY_WORD_LEN * 14
			.word mem_copy_to_ram_disk_loop + MEM_COPY_WORD_LEN * 13
			.word mem_copy_to_ram_disk_loop + MEM_COPY_WORD_LEN * 12
			.word mem_copy_to_ram_disk_loop + MEM_COPY_WORD_LEN * 11
			.word mem_copy_to_ram_disk_loop + MEM_COPY_WORD_LEN * 10
			.word mem_copy_to_ram_disk_loop + MEM_COPY_WORD_LEN * 9
			.word mem_copy_to_ram_disk_loop + MEM_COPY_WORD_LEN * 8
			.word mem_copy_to_ram_disk_loop + MEM_COPY_WORD_LEN * 7
			.word mem_copy_to_ram_disk_loop + MEM_COPY_WORD_LEN * 6
			.word mem_copy_to_ram_disk_loop + MEM_COPY_WORD_LEN * 5
			.word mem_copy_to_ram_disk_loop + MEM_COPY_WORD_LEN * 4
			.word mem_copy_to_ram_disk_loop + MEM_COPY_WORD_LEN * 3
			.word mem_copy_to_ram_disk_loop + MEM_COPY_WORD_LEN * 2
			.word mem_copy_to_ram_disk_loop + MEM_COPY_WORD_LEN * 1

; 12*4=48cc
; len = 5 bytes
.macro MEM_COPY_WORD()
			dcx h
			mov d, m
			dcx h
			mov e, m
			push d
.endmacro

;========================================
; copy a memory buffer (RAM Disk to ram )
; works with enabled interruptions
; it corrupts a pair of bytes at source addr-2
; in:
; hl - source
; de - destination
; bc - length, must be divisible by 2
; a - RAM Disk activation command

; prep: 152cc
; loop: 60-92cc
; TODO: optimization: unroll the loop 4 or more times,
; then start it depending on the remined:

.optional
mem_copy_from_ram_disk:
			shld @source + 1

			RAM_DISK_ON_BANK()
			lxi h, 0x0000
			dad sp
			shld restore_sp + 1

@source:
			lxi sp, TEMP_ADDR
			mov h, d
			mov l, e
			dad b
			xchg

			; hl - destination
			; de - destination + len
@loop2:
			mov a, e
@loop:
			; read a word
			pop b
			mov m, c
			inx h
			mov m, b
			inx h
			; check the end
			cmp l
			jnz @loop
			mov a, d
			cmp h
			jnz @loop2

			jmp restore_sp
.endopt


; Read a word from the RAM Disk w/o blocking interruptions
; It requires two reserved bytes prior the read data
; in:
; de - data addr in the RAM Disk
; a - RAM Disk activation command
; use:
; de, a
; out:
; bc - data
; de - data addr in the RAM Disk
; used: hl
; 116 cc
.optional
get_word_from_ram_disk:
			RAM_DISK_ON_BANK()
			; store sp
			lxi h, $0000
			dad sp

			; copy unpacked data into the ram_disk
			xchg
			sphl
			pop b ; bc has to be used when interruptions is on

			; restore sp
			xchg
			sphl
			RAM_DISK_OFF()
			ret
.endopt

; a special version of a func above for accessing addr $8000 and higher
; out:
; bc - data
; hl - data addr + 1 in the RAM Disk
; 100 cc
.optional
get_word_from_scr_ram_disk:
			RAM_DISK_ON_BANK()
			xchg
			mov c, m
			inx h
			mov b, m
			RAM_DISK_OFF()
			ret
.endopt


; Converts local labels to absolute by adding the absolute address
; It also can convert it back if the provided addr is negative.
; The the array data must ends with EOD word
; in:
; hl - points to the array of local ptrs to the data
; bc - the abslute offset

; out:
; hl - points to the last byte of EOD
; bc - same
; use: bc, de, hl, a
add_offset_to_labels_eod:
@loop:
			; read the local ptr
			mov e, m
			inx h
			mov d, m
			; ret if EOD
			mov a, e
			ora d
			rz
			xchg
			dad b
			xchg
			; store the offseted addr
			dcx h
			mov m, e
			inx h
			mov m, d
			inx h
			jmp @loop

; Converts local labels to absolute by adding the absolute address
; It also can convert it back if the provided addr is negative.
; in:
; hl - points to the array of ptrs to the data
; de - the abslute data addr
; c - the len of the array. if the array contains two word ptrs, then c = 2
; out:
; hl - points to the next byte after the array
; c = 0
; de - same
add_offset_to_labels_len:
@loop:
			mov a, m
			add e
			mov m, a
			inx h
			mov a, m
			adc d
			mov m, a
			inx h

			dcr c
			jnz @loop
			ret

; Set the palette
; input: hl - the addr of the last item in the palette
; use: hl, b, a

set_palette:
			hlt
set_palette_int:			; call it from an interruption routine
			mvi	a, PORT0_OUT_OUT
			out	0
			mvi	b, PALETTE_LEN - 1

@loop:		mov	a, b
			out	2
			mov a, m
			out 0x0C
			push psw
			pop psw
			push psw
			pop psw
			dcx h
			dcr b
			out 0x0C

			jp	@loop
			ret


; Copy the pallete,
; then request for using it
; in:
; hl - RAM Disk palette addr
; a - RAM Disk activation command
; uses: bc, de, hl, a
copy_palette_request_update:
			lxi d, palette
			lxi b, PALETTE_LEN
			call mem_copy_from_ram_disk

			lxi h, palette_update_request
			mvi m, PALETTE_UPD_REQ_YES
			ret


PALETTE_UPDATE_EVERY_NTH_COLOR = 2 ; update every Nth color

; Inits and performes the palette fade out
; Interrupts must be enabled
; in:
; de - palette fade addr; ex. PERMANENT_PAL_MENU_ADDR + _pal_menu_palette_fade_to_black_relative
; a - RAM Disk activation command
pallete_fade_out:
			mvi c, 0
			call pallete_fade_init
			jmp @loop
@pallete_fade_in:
			mvi c, 1
			call pallete_fade_init
@loop:
			call pallete_fade_update
			hlt
			hlt
			dcx h
			; CY=1 if the fade is complete
			jnc @loop
			ret
pallete_fade_in: = @pallete_fade_in

; Resets the fade timer
; in:
; de - palette fade addr; ex. PERMANENT_PAL_MENU_ADDR + _pal_menu_palette_fade_to_black_relative
; a - RAM Disk activation command
; c - 0 - forward fade, 1 - reverse
pallete_fade_init:
			sta pallete_fade_update_rd_cmd + 1

			; store fade direction
			lxi h, @direction + 1
			mov m, c

			; de - data addr in the RAM Disk
			; a - RAM Disk activation command
			call get_word_from_ram_disk
			; c - fade_iterations - 2
			; de - data addr in the RAM Disk

			lxi h, pallete_fade_update_iterations + 1
			mov m, c

			inx d ; advance over fade_iterations
			INX_D(SAFE_WORD_LEN) ; advance to the first fade palette

			; init the fade direction (offset to the next palette in the fade)
@direction:
			mvi a, TEMP_BYTE ; 0 - forward fade, 1 - reverse
			CPI_ZERO(NULL)
			jz @forward_fade
@reverse_fade:
			; adjust the first palette pointer
			; to the last palette in the fade
			; c - fade_iterations - 2
			xchg
			lxi d, PALETTE_LEN + SAFE_WORD_LEN
@loop:
			dad d
			dcr c
			jnz @loop
			xchg

			LXI_H_NEG(PALETTE_LEN + SAFE_WORD_LEN)
			jmp @store_palette_pointer
@forward_fade:
			; adjust the first palette pointer
			; to the last palette in the fade
			xchg
			lxi d, PALETTE_LEN + SAFE_WORD_LEN
			dad d
			xchg

@store_palette_pointer:
			shld pallete_update_next_pal_advance + 1
			xchg
			shld pallete_update_current_pal + 1
			ret

; Fades out the current pallete
; Interrupts must be enabled
; out:
; CY=1 if the fade out is complete
pallete_fade_update:
pallete_update_current_pal:
			lxi d, TEMP_ADDR
pallete_update_next_pal_advance:
			lxi h, PALETTE_LEN + SAFE_WORD_LEN
			; hl - addr offset to the next palette (PALETTE_LEN + SAFE_WORD_LEN)
			dad d
			shld pallete_update_current_pal + 1
			; de - pointer to the current palette
			; hl - pointer to the next palette
			xchg
			; hl - pointer to the current palette
pallete_fade_update_rd_cmd:
			mvi a, TEMP_BYTE
			call copy_palette_request_update

pallete_fade_update_iterations:
			mvi a, TEMP_BYTE
			sui 1
			sta pallete_fade_update_iterations + 1
			ret

; empty func
; used as a placeholder for empty callbacks
empty_func:
			ret
