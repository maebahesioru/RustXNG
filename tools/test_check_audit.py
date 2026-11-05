import unittest
from datetime import date
from check_audit import check


class AuditPolicyTests(unittest.TestCase):
    def test_vulnerabilities_never_pass_a_baseline(self):
        report = {'vulnerabilities': {'list': [{'advisory': {'id': 'A'}}]}, 'warnings': {}}
        self.assertTrue(check(report, {'expires':'2030-01-01', 'findings':[]}, date(2026,11,3)))

    def test_incomplete_or_new_findings_fail(self):
        baseline = {'expires':'2030-01-01', 'findings':[]}
        self.assertTrue(check({}, baseline, date(2026,11,3)))
        report = {'vulnerabilities':{'list':[]}, 'warnings':{'unsound':[{'advisory':{'id':'A'},'package':{'name':'p','version':'1'}}]}}
        self.assertTrue(check(report, baseline, date(2026,11,3)))
        baseline['findings'] = [dict(id='A',kind='unsound',package='p',version='1')]
        self.assertFalse(check(report, baseline, date(2026,11,3)))
        self.assertTrue(check(report, baseline, date(2030,1,1)))

    def test_an_empty_baseline_does_not_expire_or_allow_warnings(self):
        baseline = {'expires': '2026-12-03', 'findings': []}
        report = {'vulnerabilities': {'list': []}, 'warnings': {}}
        self.assertEqual(check(report, baseline, date(2027, 1, 1)), [])
        report['warnings']['unmaintained'] = [{
            'advisory': {'id': 'RUSTSEC-2024-0449'},
            'package': {'name': 'unsafe-libyaml', 'version': '0.2.11'},
        }]
        self.assertEqual(
            check(report, baseline, date(2027, 1, 1)),
            ['new warning: RUSTSEC-2024-0449 / unmaintained / unsafe-libyaml / 0.2.11'],
        )


if __name__ == '__main__':
    unittest.main()
