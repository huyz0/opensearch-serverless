# JSONTestSuite data

`jsontestsuite.json` holds the 318 parsing cases of
[JSONTestSuite](https://github.com/nst/JSONTestSuite) (`test_parsing/`), by
Nicolas Seriot. `y_` cases must be accepted, `n_` cases refused, and `i_`
cases may go either way. `tests/jsontestsuite.rs` runs them.

The cases were taken from the npm package `json-test-suite` 1.0.0 by fisker
Cheung, which carries them as JavaScript strings, and converted to JSON. Two
changes from the original files:

- The two largest cases are stored as a unit and a repeat count:
  `n_structure_100000_opening_arrays.json` and
  `n_structure_open_array_object.json`. The test rebuilds them byte for byte.
- In 25 cases (`"invalid_utf8": true`) the original invalid UTF-8 bytes did
  not survive the npm copy and show as U+FFFD. The test writes the byte 0xFF
  in each such place. These cases still test invalid UTF-8 at the same
  positions, but not the original byte sequences, which matter only to
  parsers that accept some invalid UTF-8.

## Licences

JSONTestSuite:

    MIT License

    Copyright (c) 2016 Nicolas Seriot

    Permission is hereby granted, free of charge, to any person obtaining a copy
    of this software and associated documentation files (the "Software"), to deal
    in the Software without restriction, including without limitation the rights
    to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
    copies of the Software, and to permit persons to whom the Software is
    furnished to do so, subject to the following conditions:

    The above copyright notice and this permission notice shall be included in all
    copies or substantial portions of the Software.

    THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
    IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
    FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
    AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
    LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
    OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
    SOFTWARE.

The npm package `json-test-suite`:

    MIT License

    Copyright (c) fisker Cheung <lionkay@gmail.com> (https://www.fiskercheung.com/)

    Permission is hereby granted, free of charge, to any person obtaining a copy
    of this software and associated documentation files (the "Software"), to deal
    in the Software without restriction, including without limitation the rights
    to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
    copies of the Software, and to permit persons to whom the Software is
    furnished to do so, subject to the following conditions:

    The above copyright notice and this permission notice shall be included in all
    copies or substantial portions of the Software.

    THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
    IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
    FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
    AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
    LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
    OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
    SOFTWARE.
