Feature: dbar status line

  Scenario: Clean git repository shows clean status
    Given a clean git repository
    When I run dbar status
    Then the status line contains the branch name "main"
    And the status line contains the clean glyph

  Scenario: Dirty repository shows dirty and staged markers
    Given a dirty git repository
    When I run dbar status
    Then the status line contains the dirty glyph
    And the status line contains the staged glyph
