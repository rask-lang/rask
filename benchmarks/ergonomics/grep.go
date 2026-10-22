// SPDX-License-Identifier: (MIT OR Apache-2.0)
// Go port of examples/grep_clone.rk. Same flags, output and exit codes;
// gofmt-formatted, standard library only.

package main

import (
	"errors"
	"fmt"
	"os"
	"strings"
)

type grepOptions struct {
	pattern     string
	files       []string
	ignoreCase  bool
	lineNumbers bool
	countOnly   bool
	invertMatch bool
}

func parseArgs(args []string) (grepOptions, error) {
	var opts grepOptions
	var positional []string

	for _, arg := range args[1:] {
		switch arg {
		case "-i":
			opts.ignoreCase = true
		case "-n":
			opts.lineNumbers = true
		case "-c":
			opts.countOnly = true
		case "-v":
			opts.invertMatch = true
		case "-h", "--help":
			printUsage()
			os.Exit(0)
		case "--":
		default:
			positional = append(positional, arg)
		}
	}

	if len(positional) == 0 {
		return opts, errors.New("missing pattern argument")
	}
	if len(positional) < 2 {
		return opts, errors.New("no files specified")
	}

	opts.pattern = positional[0]
	opts.files = positional[1:]
	return opts, nil
}

func printUsage() {
	fmt.Println("Usage: rgrep [OPTIONS] PATTERN FILE...")
	fmt.Println("")
	fmt.Println("Options:")
	fmt.Println("  -i    Ignore case")
	fmt.Println("  -n    Show line numbers")
	fmt.Println("  -c    Count matching lines only")
	fmt.Println("  -v    Invert match (show non-matching lines)")
	fmt.Println("  -h    Show this help")
}

func lineMatches(line, pattern string, ignoreCase bool) bool {
	if ignoreCase {
		return strings.Contains(strings.ToLower(line), strings.ToLower(pattern))
	}
	return strings.Contains(line, pattern)
}

func grepFile(path string, opts grepOptions) (int, error) {
	content, err := os.ReadFile(path)
	if err != nil {
		return 0, err
	}

	matchCount := 0
	for i, line := range strings.Split(strings.TrimSuffix(string(content), "\n"), "\n") {
		matches := lineMatches(line, opts.pattern, opts.ignoreCase)
		if matches == opts.invertMatch {
			continue
		}
		matchCount++
		if opts.countOnly {
			continue
		}
		if opts.lineNumbers {
			fmt.Printf("%d:%s\n", i+1, line)
		} else {
			fmt.Println(line)
		}
	}

	if opts.countOnly {
		fmt.Println(matchCount)
	}
	return matchCount, nil
}

func main() {
	opts, err := parseArgs(os.Args)
	if err != nil {
		fmt.Printf("rgrep: %v\n", err)
		printUsage()
		os.Exit(1)
	}

	totalMatches := 0
	hadErrors := false
	for _, path := range opts.files {
		found, err := grepFile(path, opts)
		if err != nil {
			fmt.Printf("rgrep: %v\n", err)
			hadErrors = true
			continue
		}
		totalMatches += found
	}

	if hadErrors {
		os.Exit(2)
	}
	if totalMatches == 0 {
		os.Exit(1)
	}
}
