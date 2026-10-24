// SPDX-License-Identifier: (MIT OR Apache-2.0)
// Go port of examples/grep_clone.rk. Same flags, output and exit codes;
// gofmt-formatted, standard library only.

package main

import (
	"flag"
	"fmt"
	"os"
	"strings"
)

type grepOptions struct {
	pattern     string
	ignoreCase  bool
	lineNumbers bool
	countOnly   bool
	invertMatch bool
}

func lineMatches(line string, opts grepOptions) bool {
	if opts.ignoreCase {
		return strings.Contains(strings.ToLower(line), strings.ToLower(opts.pattern))
	}
	return strings.Contains(line, opts.pattern)
}

func grepFile(path string, opts grepOptions) (int, error) {
	content, err := os.ReadFile(path)
	if err != nil {
		return 0, err
	}

	matchCount := 0
	for i, line := range strings.Split(strings.TrimSuffix(string(content), "\n"), "\n") {
		if lineMatches(line, opts) == opts.invertMatch {
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
	var opts grepOptions
	flag.BoolVar(&opts.ignoreCase, "i", false, "Ignore case")
	flag.BoolVar(&opts.lineNumbers, "n", false, "Show line numbers")
	flag.BoolVar(&opts.countOnly, "c", false, "Count matching lines only")
	flag.BoolVar(&opts.invertMatch, "v", false, "Show non-matching lines")
	flag.Parse()

	if flag.NArg() < 2 {
		fmt.Println("rgrep: need a pattern and at least one file")
		os.Exit(2)
	}
	opts.pattern = flag.Arg(0)

	totalMatches := 0
	hadErrors := false
	for _, path := range flag.Args()[1:] {
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
