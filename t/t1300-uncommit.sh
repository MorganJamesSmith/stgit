#!/bin/sh
#
# Copyright (c) 2006 Catalin Marinas
#

test_description='Test the uncommit command.

'

. ./test-lib.sh

test_expect_success \
	'Initialize the StGit repository' \
	'stg init
'

test_expect_success \
	'Create the first patch' \
	'
	stg new foo -m "Foo Patch" &&
	echo foo > test &&
	stg add test &&
	stg refresh
	'

test_expect_success \
	'Create the second patch' \
	'
	stg new bar -m "Bar Patch" &&
	echo bar > test &&
	stg add test &&
	stg refresh
	'

test_expect_success \
	'Commit the patches' \
	'
	stg commit --all
	'

if test -n "$STG_TEST_PYTHON"; then
test_expect_success \
  'Invalid --to and --number arguments' \
  '
  command_error stg uncommit --to HEAD^ --number 1 2>err &&
  grep -e "cannot give both --to and --number" err
  '
else
test_expect_success \
  'Invalid --to and --number arguments' \
  '
  general_error stg uncommit --to HEAD^ --number 1 2>err &&
  grep -e "error: The argument .--to <commitish>. cannot be used with .--number <number>." err
  '
fi

if test -n "$STG_TEST_PYTHON"; then
test_expect_success \
  'Invalid --to with patch args' \
  '
  command_error stg uncommit --to HEAD^ p0 2>err &&
  grep -e "cannot specify patch name with --to" err
  '
else
test_expect_success \
  'Invalid --to with patch args' \
  '
  general_error stg uncommit --to HEAD^ p0 2>err &&
  grep -e "error: The argument .--to <commitish>. cannot be used with .<patchname>\.\.\.." err
  '
fi

if test -n "$STG_TEST_PYTHON"; then
test_expect_success \
  'Invalid --number' \
  '
  command_error stg uncommit --number -1 2>err &&
  grep -e "invalid value passed to --number" err
  '
else
test_expect_success \
  'Invalid --number' \
  '
  general_error stg uncommit --number -1 2>err &&
  grep -e "Invalid value \"-1\" for .--number <number>.: .-1. is not a positive integer" err
  '
fi

if test -n "$STG_TEST_PYTHON"; then
test_expect_success \
  'Too many patch names with --number' \
  '
  command_error stg uncommit --number 2 p0 p1 2>err &&
  grep -e "when using --number, specify at most one patch name" err
  '
else
test_expect_success \
  'Too many patch names with --number' \
  '
  command_error stg uncommit --number 2 p0 p1 2>err &&
  grep -e "When using \`--number\`, specify at most one patch name" err
  '
fi

test_expect_success \
	'Uncommit the patches using names' \
	'
	stg uncommit bar foo &&
	[ "$(stg id foo)" = "$(stg id bar^)" ] &&
	stg commit --all
	'

test_expect_success \
	'Uncommit the patches using prefix' \
	'
	stg uncommit --number=2 foobar &&
	[ "$(stg id foobar1)" = "$(stg id foobar2^)" ] &&
	stg commit --all
	'

test_expect_success \
	'Uncommit the patches using auto names' \
	'
	stg uncommit --number=2 &&
	[ "$(stg id foo-patch)" = "$(stg id bar-patch^)" ] &&
	stg commit --all
	'

test_expect_success \
	'Uncommit the patches one by one' \
	'
	stg uncommit &&
	stg uncommit &&
	[ "$(stg id foo-patch)" = "$(stg id bar-patch^)" ] &&
	stg commit --all
	'

test_expect_success \
    'Uncommit the patches with --to' '
    stg uncommit --to HEAD^ &&
    [ "$(stg id foo-patch)" = "$(stg id bar-patch^)" ] &&
    stg commit --all
'

test_expect_success \
  'Use --exclusive' \
  '
  stg uncommit --to HEAD^ --exclusive &&
  [ "$(echo $(stg series --applied --noprefix))" = "bar-patch" ] &&
  stg commit --all
  '

if test -n "$STG_TEST_PYTHON"; then
test_expect_success 'Attempt to reuse patch name' '
  stg uncommit &&
  [ "$(echo $(stg series --applied --noprefix))" = "bar-patch" ] &&
  command_error stg uncommit bar-patch 2>err &&
  grep -e "Patch name \"bar-patch\" already taken" err &&
  stg commit --all
'
else
test_expect_success 'Attempt to reuse patch name' '
  stg uncommit &&
  [ "$(echo $(stg series --applied --noprefix))" = "bar-patch" ] &&
  command_error stg uncommit bar-patch 2>err &&
  grep -e "Patch \`bar-patch\` already exists" err &&
  stg commit --all
'
fi

if test -n "$STG_TEST_PYTHON"; then
test_expect_success 'Attempt to use invalid patch name' '
  command_error stg uncommit bad..patchname
'
else
test_expect_success 'Attempt to use invalid patch name' '
  general_error stg uncommit bad..patchname 2>err &&
  grep -e "error: Invalid value \"bad\.\.patchname\" for .<patchname>\.\.\.." err
'
fi

test_expect_success 'Uncommit a commit with not precisely one parent' '
    command_error stg uncommit -n 5  &&
    [ "$(echo $(stg series))" = "" ]
'

# stg uncommit should work even when top != head, and should not touch
# the head.
test_expect_success 'Uncommit when top != head' '
    stg new -m foo &&
    git reset --hard HEAD^ &&
    h=$(git rev-parse HEAD)
    stg uncommit bar &&
    test "$(git rev-parse HEAD)" = "$h" &&
    test "$(echo $(stg series))" = "+ bar > foo"
'

test_done
